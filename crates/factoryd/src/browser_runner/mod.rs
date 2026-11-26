//! Claktory's browser runner (#435, agent-browser.md Rev 2 R1–R2):
//! `factoryd browser-runner`, the process that keeps Claktory's own Chrome
//! on a private screen. On a Linux server factoryd runs it as the browser's
//! own user through sudo (`sudo -n -u claktory-browser
//! /opt/claktory-browser/runner browser-runner run`), and on a Mac inside a
//! small Linux VM. It talks to factoryd only over its stdin and stdout
//! (`wire`, C1): no socket, for DevTools or anything else.
//!
//! - `run`: first its own namespaces and /proc (`namespace`), then it waits
//!   for `start`, makes the screen (`xvfb`) and Chrome (`chrome`) on the
//!   kept profile, fenced by Landlock (`fence`), and relays DevTools while
//!   Chrome is driven. Chrome is quit as a person would at every stop.
//! - `selftest`: what Chrome, fenced, can't reach (`selftest`).
//! - `abi`: the kernel's Landlock ABI.
//!
//! Profiles live in `/var/lib/claktory-browser/profiles/u<uid>-<name>`,
//! the uid sudo says ran it (SUDO_UID), so one person's is never another's.
//! Off for everyone until slice 1 ships: nothing starts it yet.

// The Mac builds the runner (so its clippy reads it) but never runs it.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

pub mod wire;

mod capture;
mod chrome;
mod clipboard;
mod downloads;
#[cfg(target_os = "linux")]
mod fence;
mod input;
mod keysym;
mod live;
#[cfg(target_os = "linux")]
mod namespace;
#[cfg(target_os = "linux")]
mod selftest;
pub mod xvfb;

use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use wire::{Channel, Control, Mode, Report, Way};

/// Where the kept profiles are: the browser user's own folder (setup makes
/// it, 0700).
pub(crate) const PROFILES: &str = "/var/lib/claktory-browser/profiles";

/// What the runner's threads tell its loop.
pub(crate) enum Event {
    Control(Control),
    /// A control message that didn't read: dropped, and said.
    BadControl,
    /// A DevTools message for Chrome.
    Cdp(Vec<u8>),
    /// A frame's ack, input or clipboard text: the screen's (P2).
    Screen(Channel, Vec<u8>),
    /// stdin ended, or broke the wire (Some: how).
    Closed(Option<String>),
    /// A DevTools message from the Chrome of this generation.
    FromChrome(u64, Vec<u8>),
    /// That Chrome said more than MAX_CDP in one message: dropped.
    Oversized(u64),
    /// That Chrome ended, with its exit code.
    ChromeExited(u64, Option<i32>),
}

/// What goes out on stdout, a frame at a time, from any thread.
#[derive(Clone)]
pub struct Outbox {
    to: Sender<(Channel, Vec<u8>)>,
    /// Channel 5's bytes queued and not written yet.
    files: downloads::Waiting,
}

impl Outbox {
    pub(crate) fn new(to: Sender<(Channel, Vec<u8>)>) -> Outbox {
        Outbox { to, files: downloads::Waiting::default() }
    }

    pub fn send(&self, channel: Channel, payload: Vec<u8>) {
        if channel == Channel::Files {
            self.files.add(payload.len());
        }
        let _ = self.to.send((channel, payload));
    }

    /// A frame of `bytes` on `channel` was written out.
    pub(crate) fn sent(&self, channel: Channel, bytes: usize) {
        if channel == Channel::Files {
            self.files.sent(bytes);
        }
    }

    /// Channel 5's bytes still to go out.
    pub(crate) fn files_waiting(&self) -> u64 {
        self.files.get()
    }

    pub fn report(&self, report: &Report) {
        self.send(Channel::Control, wire::encode(report));
    }
}

/// `factoryd browser-runner …`.
pub fn main(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("run") if args.len() == 1 => run(),
        #[cfg(target_os = "linux")]
        Some("selftest") => selftest::main(&args[1..]),
        #[cfg(target_os = "linux")]
        Some("abi") => {
            println!("{}", fence::abi());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: factoryd browser-runner run | selftest | abi (Linux only)");
            ExitCode::from(2)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run() -> ExitCode {
    eprintln!("Claktory's browser runner runs on Linux only.");
    ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
fn run() -> ExitCode {
    if let Err(why) = namespace::enter() {
        fatal_before_start(&why);
        return ExitCode::FAILURE;
    }
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: a handler that only stores a flag.
        unsafe { libc::signal(signal, stop_asked as *const () as libc::sighandler_t) };
    }
    Runner::new().run()
}

static STOP_ASKED: AtomicBool = AtomicBool::new(false);

extern "C" fn stop_asked(_: libc::c_int) {
    STOP_ASKED.store(true, Ordering::Relaxed);
}

/// Says why on stdout, as the wire does, before any thread is there to.
#[cfg(target_os = "linux")]
fn fatal_before_start(why: &str) {
    use std::io::Write;
    let report = wire::encode(&Report::Error { what: why.to_owned(), fatal: true });
    let mut out = std::io::stdout().lock();
    let _ = wire::write(&mut out, Channel::Control, &report, Way::Out);
    let _ = out.flush();
}

/// The started screen and Chrome, on one kept profile.
struct Session {
    name: String,
    /// `PROFILES/u<uid>-<name>`: Chrome's data, its HOME and its last log.
    root: PathBuf,
    /// The runner's private folder for this session (0700): the screen's
    /// cookie and logs.
    tmp: PathBuf,
    screen: xvfb::Screen,
    /// The live view of the screen: frames, the person's input and the
    /// clipboard (P2); None when it couldn't start, which was said.
    live: Option<live::Live>,
    chrome: Option<chrome::Chrome>,
    binary: PathBuf,
    /// The profile's lock, held while the session lasts.
    #[allow(dead_code)]
    lock: std::fs::File,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fence_version: u32,
    /// The person's time zone and languages, checked.
    person: chrome::Person,
    /// Where a driven errand's downloads land (R9); None when the folder
    /// couldn't be made, which was said.
    downloads: Option<downloads::Downloads>,
    width: u16,
    height: u16,
}

impl Session {
    fn profile(&self) -> PathBuf {
        self.root.join("chrome")
    }
}

struct Runner {
    out: Outbox,
    events: Receiver<Event>,
    tell: Sender<Event>,
    uid: u32,
    session: Option<Session>,
    /// Each Chrome's number, so what an earlier one says is told apart.
    generation: u64,
}

impl Runner {
    fn new() -> Runner {
        let (outbox, outgoing) = mpsc::channel::<(Channel, Vec<u8>)>();
        let out = Outbox::new(outbox);
        let (tell, events) = mpsc::channel();
        std::thread::spawn({
            let (tell, out) = (tell.clone(), out.clone());
            move || write_out(outgoing, tell, out)
        });
        std::thread::spawn({
            let tell = tell.clone();
            move || read_in(tell)
        });
        // The person who ran it: sudo says (and can't be told otherwise);
        // run directly, this user.
        // SAFETY: getuid can't fail.
        let own = unsafe { libc::getuid() };
        let uid = std::env::var("SUDO_UID").ok().and_then(|uid| uid.parse().ok()).unwrap_or(own);
        Runner { out, events, tell, uid, session: None, generation: 0 }
    }

    fn run(mut self) -> ExitCode {
        loop {
            if STOP_ASKED.load(Ordering::Relaxed) {
                self.stop();
                return ExitCode::SUCCESS;
            }
            self.downloads_tick();
            let event = match self.events.recv_timeout(Duration::from_secs(1)) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => {
                    reap_orphans(self.session.as_ref());
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.stop();
                    return ExitCode::SUCCESS;
                }
            };
            match event {
                Event::Control(control) => {
                    if self.control(control) {
                        return ExitCode::SUCCESS;
                    }
                }
                Event::BadControl => self.error("a control message that doesn't read was dropped"),
                Event::Cdp(message) => {
                    // Undriven, Chrome has no pipe: there's nowhere for it to go.
                    if let Some(chrome) = self.session.as_mut().and_then(|session| session.chrome.as_mut())
                        && chrome.mode == Mode::Driven
                        && let Err(why) = chrome.send(&message)
                    {
                        self.error(&why);
                    }
                }
                Event::Screen(channel, payload) => {
                    if let Some(live) = self.session.as_ref().and_then(|session| session.live.as_ref()) {
                        live.screen(channel, payload);
                    }
                }
                Event::Closed(why) => {
                    if let Some(why) = why {
                        self.error(&format!("factoryd's messages broke off: {why}"));
                    }
                    self.stop();
                    return ExitCode::SUCCESS;
                }
                Event::FromChrome(generation, message) => {
                    if generation == self.generation && !self.downloads_seen(&message) {
                        self.out.send(Channel::Cdp, message);
                    }
                }
                Event::Oversized(generation) => {
                    if generation == self.generation {
                        self.error("Chrome said more than 8 MiB in one DevTools message, so it was dropped");
                    }
                }
                Event::ChromeExited(generation, code) => {
                    if generation == self.generation
                        && let Some(session) = &mut self.session
                        && session.chrome.as_mut().is_some_and(|chrome| chrome.ended())
                    {
                        session.chrome = None;
                        self.out.report(&Report::ChromeExited { code });
                        reap_orphans(self.session.as_ref());
                    }
                }
            }
        }
    }

    /// One control message; true when the runner ends after it.
    fn control(&mut self, control: Control) -> bool {
        match control {
            Control::Start { profile, mode, url, width, height, time_zone, languages } => {
                if self.session.is_some() {
                    self.error("the browser has started already");
                    return false;
                }
                let person = self.person(time_zone.as_deref(), languages);
                match self.start(&profile, mode, url.as_deref(), width, height, person) {
                    Ok(report) => self.out.report(&report),
                    Err(why) => {
                        self.out.report(&Report::Error { what: why, fatal: true });
                        self.stop();
                        return true;
                    }
                }
            }
            Control::Mode { mode, url } => {
                let Some(session) = &mut self.session else {
                    self.error("start the browser first");
                    return false;
                };
                if let Some(chrome) = &mut session.chrome {
                    chrome.quit(&session.screen.display);
                }
                session.chrome = None;
                // What the person pasted or copied stays theirs: emptied
                // before Chrome starts again, driven or not, or it doesn't
                // start (Fugo 36828).
                // So are downloads: none crosses from an errand to the
                // person's turn or back (Fugo 36912).
                if let Some(downloads) = &mut session.downloads {
                    downloads.empty();
                }
                if let Some(Err(why)) = session.live.as_ref().map(live::Live::forget_clipboard) {
                    self.out.report(&Report::Error { what: format!("couldn't empty the browser's clipboard, so Chrome wasn't started again: {why}"), fatal: false });
                    return false;
                }
                self.generation += 1;
                match launch(session, mode, url.as_deref(), self.tell.clone(), self.generation) {
                    Ok(chrome) => {
                        session.chrome = Some(chrome);
                        self.out.report(&Report::ModeChanged { mode });
                    }
                    Err(why) => self.error(&why),
                }
            }
            Control::Stop => {
                self.stop();
                return true;
            }
            Control::DeleteData { profile } => {
                if !wire::profile_name_ok(&profile) {
                    self.error("that isn't a profile's name");
                    return false;
                }
                if let Some(session) = &self.session
                    && session.name != profile
                {
                    self.error("that isn't the profile running");
                    return false;
                }
                self.stop_session();
                let root = self.root(&profile);
                // Never under another runner's Chrome.
                let _lock = match lock_profile(&root) {
                    Ok(lock) => Some(lock),
                    Err(why) if root.exists() => {
                        self.error(&format!("couldn't delete the browser's data: {why}"));
                        self.out.report(&Report::Stopped { ms: 0 });
                        return true;
                    }
                    Err(_) => None,
                };
                match std::fs::remove_dir_all(&root) {
                    Ok(()) => self.out.report(&Report::DataDeleted),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => self.out.report(&Report::DataDeleted),
                    Err(error) => self.error(&format!("couldn't delete the browser's data: {error}")),
                }
                self.out.report(&Report::Stopped { ms: 0 });
                return true;
            }
            Control::View { on, max_width, quality_hint } => {
                if let Some(live) = self.session.as_ref().and_then(|session| session.live.as_ref()) {
                    live.view(on, max_width, quality_hint);
                }
            }
            Control::Resize { width, height } => {
                let Some(session) = &mut self.session else {
                    self.error("start the browser first");
                    return false;
                };
                let resized = match &session.live {
                    Some(live) => live.resize(width, height),
                    None => Err("the live view isn't running".to_owned()),
                };
                match resized {
                    Ok(()) => {
                        // Chrome starts again at this size after a change of mode.
                        (session.width, session.height) = (width, height);
                        self.out.report(&Report::Resized { width, height });
                    }
                    Err(why) => self.error(&format!("couldn't resize the browser: {why}")),
                }
            }
            Control::Clipboard { watch } => {
                if let Some(live) = self.session.as_ref().and_then(|session| session.live.as_ref()) {
                    live.watch_clipboard(watch);
                }
            }
            // R9: only a driven errand's downloads are read; any is dropped.
            Control::DownloadRead { guid } => {
                let driven = self.session.as_ref().and_then(|session| session.chrome.as_ref()).is_some_and(|chrome| chrome.mode == Mode::Driven);
                match self.session.as_mut().and_then(|session| session.downloads.as_mut()) {
                    Some(downloads) if driven => downloads.read(&guid, &self.out),
                    _ => self.out.report(&Report::DownloadCancelled { guid, why: "the browser isn't driven, so nothing can be downloaded".to_owned() }),
                }
            }
            Control::DownloadDrop { guid } => {
                if let Some(downloads) = self.session.as_mut().and_then(|session| session.downloads.as_mut()) {
                    downloads.delete(&guid);
                }
            }
        }
        false
    }

    fn root(&self, profile: &str) -> PathBuf {
        Path::new(PROFILES).join(format!("u{}-{profile}", self.uid))
    }

    fn start(&mut self, profile: &str, mode: Mode, url: Option<&str>, width: u16, height: u16, person: chrome::Person) -> Result<Report, String> {
        if !wire::profile_name_ok(profile) {
            return Err("that isn't a profile's name".to_owned());
        }
        if !(320..=4096).contains(&width) || !(320..=4096).contains(&height) {
            return Err("the screen must be 320 to 4096 pixels each way".to_owned());
        }
        if let Some(url) = url {
            chrome::checked_url(url)?;
        }
        firewall_holds()?;
        let root = self.root(profile);
        private_folder(&root).map_err(|error| format!("couldn't make the browser's profile: {error}"))?;
        let lock = lock_profile(&root)?;
        let home = root.join("home");
        private_folder(&home).map_err(|error| format!("couldn't make the browser's profile: {error}"))?;
        chrome::first_preferences(&root.join("chrome")).map_err(|error| format!("couldn't make the browser's profile: {error}"))?;
        let binary = chrome::binary()?;
        let tmp = own_tmp()?;
        let screen = match xvfb::Screen::start(&tmp, width, height) {
            Ok(screen) => screen,
            Err(why) => {
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(why);
            }
        };
        let mut session =
            Session { name: profile.to_owned(), root, tmp, screen, live: None, chrome: None, binary, lock, fence_version: 0, person, downloads: None, width, height };
        match downloads::Downloads::new(&session.tmp) {
            Ok(downloads) => session.downloads = Some(downloads),
            Err(error) => self.error(&format!("the browser's downloads folder couldn't be made, so nothing can be downloaded: {error}")),
        }
        self.generation += 1;
        match launch(&mut session, mode, url, self.tell.clone(), self.generation) {
            Ok(chrome) => session.chrome = Some(chrome),
            Err(why) => {
                finish(session);
                return Err(why);
            }
        }
        // The browser works without its live view, if not as well: said,
        // and carried on.
        match live::Live::start(&session.screen.display, width, height, &self.out) {
            Ok(live) => session.live = Some(live),
            Err(why) => self.error(&format!("the browser's live view couldn't start: {why}")),
        }
        let download_dir = session.downloads.as_ref().map(downloads::Downloads::dir);
        let report = Report::Ready { wire: wire::VERSION, chrome: chrome::version(&session.binary), landlock_abi: session.fence_version, mode, download_dir };
        self.session = Some(session);
        Ok(report)
    }

    /// Quits Chrome as a person would, then the screen.
    fn stop_session(&mut self) -> u64 {
        let started = Instant::now();
        if let Some(session) = self.session.take() {
            finish(session);
        }
        started.elapsed().as_millis() as u64
    }

    fn stop(&mut self) {
        let ms = self.stop_session();
        self.out.report(&Report::Stopped { ms });
        // What's queued goes out before the runner ends.
        std::thread::sleep(Duration::from_millis(100));
    }

    /// Chrome's download events, held to the caps as they pass (R9);
    /// true for a message that stays here (an answer to the runner's own
    /// cancel).
    fn downloads_seen(&mut self, message: &[u8]) -> bool {
        let Some(downloads) = self.session.as_mut().and_then(|session| session.downloads.as_mut()) else { return false };
        let seen = downloads.seen(message);
        self.cancel_downloads(seen.cancel);
        seen.own
    }

    /// While downloads run: all cancelled when the disk runs short.
    fn downloads_tick(&mut self) {
        let Some(downloads) = self.session.as_mut().and_then(|session| session.downloads.as_mut()) else { return };
        let cancel = downloads.tick();
        self.cancel_downloads(cancel);
    }

    /// Chrome told to cancel each, its file deleted, and factoryd told why.
    fn cancel_downloads(&mut self, cancel: Vec<(String, String)>) {
        let Some(session) = self.session.as_mut() else { return };
        for (guid, why) in cancel {
            if let (Some(downloads), Some(chrome)) = (session.downloads.as_mut(), session.chrome.as_mut()) {
                let call = downloads.cancel_call(&guid);
                if chrome.mode == Mode::Driven {
                    let _ = chrome.send(&call);
                }
                downloads.delete(&guid);
            }
            self.out.report(&Report::DownloadCancelled { guid, why });
        }
    }

    fn error(&self, what: &str) {
        self.out.report(&Report::Error { what: what.to_owned(), fatal: false });
    }

    /// The time zone and languages the browser takes (change 3): the
    /// person's when they're well formed and this computer knows the zone;
    /// otherwise UTC or en-US, said in an error that doesn't end it (Fugo
    /// 37383). Nothing that fails a check reaches Chrome.
    fn person(&self, time_zone: Option<&str>, languages: Vec<String>) -> chrome::Person {
        let mut person = chrome::Person::default();
        if let Some(zone) = time_zone {
            match chrome::zone_folder(zone) {
                Some(folder) => (person.time_zone, person.zone_folder) = (zone.to_owned(), folder),
                None => self.error("the person's time zone isn't one this computer knows, so the browser uses UTC"),
            }
        }
        if !languages.is_empty() {
            if languages.len() <= wire::MAX_LANGUAGES && languages.iter().all(|tag| wire::language_ok(tag)) {
                person.languages = languages;
            } else {
                self.error("the person's languages aren't ones the browser takes, so it uses en-US");
            }
        }
        person
    }
}

/// Starts Chrome on the session's screen and profile, fenced.
fn launch(session: &mut Session, mode: Mode, url: Option<&str>, tell: Sender<Event>, generation: u64) -> Result<chrome::Chrome, String> {
    let profile = session.profile();
    let fence = fence_for(session, &profile)?;
    let home = session.root.join("home");
    let launch = chrome::Launch {
        binary: &session.binary,
        profile: &profile,
        home: &home,
        tmp: &session.tmp,
        display: &session.screen.display,
        person: &session.person,
        mode,
        url,
        width: session.width,
        height: session.height,
        fence: fence.as_ref(),
    };
    chrome::Chrome::launch(&launch, tell, generation)
}

#[cfg(target_os = "linux")]
fn fence_for(session: &mut Session, _profile: &Path) -> Result<Option<OwnedFd>, String> {
    let (rules, version) = fence::rules(&fence::read_write(&session.root, &session.tmp), &fence::read_only())?;
    session.fence_version = version;
    Ok(Some(xvfb::above_stdio(rules)?))
}

#[cfg(not(target_os = "linux"))]
fn fence_for(_session: &mut Session, _profile: &Path) -> Result<Option<OwnedFd>, String> {
    Err("Claktory's browser runs on Linux only".to_owned())
}

/// Whether the firewall setup loads (Pericolo 36665, Fugo 36669, 36785)
/// still holds: the browser's user mustn't reach this machine, by loopback
/// or by any of its own addresses (a private one on a LAN, say), v4 or v6.
/// So the runner, as that user, listens on a port of its own on every
/// address and must be refused at each: a connection that goes through
/// means the rule isn't loaded (a `systemctl stop nftables` flushes it),
/// and nothing starts.
#[cfg(target_os = "linux")]
pub(crate) fn firewall_holds() -> Result<(), String> {
    use std::net::{SocketAddr, TcpListener, TcpStream};
    let checking = |error: std::io::Error| format!("couldn't check the browser's firewall: {error}");
    // A listener for each stack, IPv6's taking IPv6 only whatever
    // net.ipv6.bindv6only says: a v4 connection reaches only the v4 one, so
    // a v6-only [::] can't refuse v4 for the firewall (Fugo 37547). IPv4
    // alone where the machine has no IPv6.
    let v4 = TcpListener::bind("0.0.0.0:0").map_err(checking)?;
    let v6 = v6_only_listener();
    let v4_port = v4.local_addr().map_err(checking)?.port();
    let v6_port = v6.as_ref().map(TcpListener::local_addr).transpose().map_err(checking)?.map(|address: SocketAddr| address.port());
    let own = own_addresses();
    // IPv6 here that couldn't be checked fails closed (Fugo 37680).
    if v6_port.is_none() && targets(0, Some(0), own.clone()).iter().any(|address| address.is_ipv6() && !address.ip().is_loopback()) {
        return Err("couldn't check the browser's firewall: this computer has IPv6 addresses, but no IPv6 listener could be made to check them".to_owned());
    }
    for address in targets(v4_port, v6_port, own) {
        if TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok() {
            return Err(format!(
                "the browser's firewall isn't loaded, so its pages could reach this computer (at {}): run Claktory's browser setup again",
                address.ip()
            ));
        }
    }
    Ok(())
}

/// Where the firewall check connects: loopback and each of this machine's
/// own addresses, a v4 one at the v4 listener's port and a v6 one at the
/// v6 listener's (none without it; link-local needs a scope to reach).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn targets(v4_port: u16, v6_port: Option<u16>, own: Vec<std::net::IpAddr>) -> Vec<std::net::SocketAddr> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    let mut addresses = vec![IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)];
    addresses.extend(own.into_iter().filter(|address| !address.is_loopback()));
    addresses
        .into_iter()
        .filter_map(|address| match address {
            IpAddr::V4(_) => Some(SocketAddr::new(address, v4_port)),
            IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) != 0xfe80 => v6_port.map(|port| SocketAddr::new(address, port)),
            IpAddr::V6(_) => None,
        })
        .collect()
}

/// A listener on [::] that takes IPv6 only (IPV6_V6ONLY set before it's
/// bound), on a port of its own; None where the machine has no IPv6.
#[cfg(target_os = "linux")]
fn v6_only_listener() -> Option<std::net::TcpListener> {
    use std::os::fd::{AsRawFd, FromRawFd};
    // SAFETY: a socket made here, owned at once; setsockopt, bind and listen
    // on it with values that live across each call.
    unsafe {
        let fd = libc::socket(libc::AF_INET6, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return None;
        }
        let fd = OwnedFd::from_raw_fd(fd);
        let on: libc::c_int = 1;
        let length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        if libc::setsockopt(fd.as_raw_fd(), libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, (&raw const on).cast(), length) != 0 {
            return None;
        }
        let mut any: libc::sockaddr_in6 = std::mem::zeroed();
        any.sin6_family = libc::AF_INET6 as libc::sa_family_t;
        let size = std::mem::size_of::<libc::sockaddr_in6>() as libc::socklen_t;
        if libc::bind(fd.as_raw_fd(), (&raw const any).cast(), size) != 0 || libc::listen(fd.as_raw_fd(), 8) != 0 {
            return None;
        }
        Some(std::net::TcpListener::from(fd))
    }
}

/// This machine's own addresses, from its interfaces.
#[cfg(target_os = "linux")]
fn own_addresses() -> Vec<std::net::IpAddr> {
    let mut found = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills the list, read here and freed once.
    unsafe {
        if libc::getifaddrs(&mut list) != 0 {
            return found;
        }
        let mut at = list;
        while !at.is_null() {
            let address = (*at).ifa_addr;
            if !address.is_null() {
                match i32::from((*address).sa_family) {
                    libc::AF_INET => {
                        let v4 = &*(address as *const libc::sockaddr_in);
                        found.push(std::net::IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(v4.sin_addr.s_addr))));
                    }
                    libc::AF_INET6 => {
                        let v6 = &*(address as *const libc::sockaddr_in6);
                        found.push(std::net::IpAddr::V6(std::net::Ipv6Addr::from(v6.sin6_addr.s6_addr)));
                    }
                    _ => {}
                }
            }
            at = (*at).ifa_next;
        }
        libc::freeifaddrs(list);
    }
    found
}

#[cfg(not(target_os = "linux"))]
fn firewall_holds() -> Result<(), String> {
    Err("Claktory's browser runs on Linux only".to_owned())
}

/// Ends a session: Chrome quit as a person would, its log's end kept with
/// the profile, the screen stopped, the private folder gone.
fn finish(mut session: Session) {
    // Input half typed stops before Chrome is asked to quit.
    session.live = None;
    if let Some(chrome) = &mut session.chrome {
        chrome.quit(&session.screen.display);
    }
    session.chrome = None;
    if let Ok(log) = std::fs::read(session.tmp.join("chrome.log")) {
        let tail = &log[log.len().saturating_sub(4096)..];
        let _ = std::fs::write(session.root.join("chrome-last.log"), tail);
    }
    session.screen.stop();
    let _ = std::fs::remove_dir_all(&session.tmp);
}

/// Applies the fence made beforehand to this process, in Chrome's child
/// between fork and exec: only async-signal-safe calls. Landlock needs
/// no_new_privs, which the runner has already.
#[cfg(target_os = "linux")]
pub(crate) fn fence_self(fence: std::os::fd::RawFd) -> std::io::Result<()> {
    // SAFETY: prctl and landlock_restrict_self on this process, with an fd
    // that stays open until exec.
    unsafe {
        if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 || libc::syscall(libc::SYS_landlock_restrict_self, fence, 0u32) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn fence_self(_fence: std::os::fd::RawFd) -> std::io::Result<()> {
    Err(std::io::Error::other("Landlock is Linux's"))
}

/// In a child between fork and exec: ends with the runner, killed or not.
pub(crate) fn die_with_parent() {
    #[cfg(target_os = "linux")]
    // SAFETY: a prctl on this process.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0);
    }
}

/// Ends a child: SIGTERM, then SIGKILL after `within`.
pub(crate) fn end(child: &mut Child, within: Duration) {
    // SAFETY: our own child, not yet reaped.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// The profile's lock: held by one runner at a time, so a second one (a
/// runner its factoryd lost track of, say) never starts another Chrome on
/// the same profile. Released when the runner ends, however it ends.
fn lock_profile(root: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root.join("runner.lock"))
        .map_err(|error| format!("couldn't lock the browser's profile: {error}"))?;
    // SAFETY: flock on a descriptor we own.
    if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&file), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Claktory's browser is already running on this profile, in another runner".to_owned());
    }
    Ok(file)
}

/// A folder only the runner's user reads (0700), made if it isn't there;
/// a link where it should be is refused.
pub(crate) fn private_folder(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match std::fs::symlink_metadata(path) {
        Ok(found) if found.is_dir() => {
            if found.permissions().mode() & 0o077 != 0 {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Ok(_) => Err(std::io::Error::other(format!("{} isn't a folder", path.display()))),
        Err(_) => std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path),
    }
}

/// A new 0700 folder in /tmp, only this session's.
fn own_tmp() -> Result<PathBuf, String> {
    use std::os::unix::fs::DirBuilderExt;
    let mut name = [0u8; 8];
    getrandom::fill(&mut name).map_err(|error| format!("no random name: {error}"))?;
    let hex: String = name.iter().map(|byte| format!("{byte:02x}")).collect();
    let path = std::env::temp_dir().join(format!("claktory-browser-{hex}"));
    std::fs::DirBuilder::new().mode(0o700).create(&path).map_err(|error| format!("couldn't make the browser's folder: {error}"))?;
    Ok(path)
}

/// The runner is PID 1 of its namespace: processes of Chrome's whose
/// parent died come to it, and it reaps them (never the screen or Chrome,
/// whose own waits do).
fn reap_orphans(session: Option<&Session>) {
    let keep: Vec<u32> = session.map(|session| {
        let mut keep = vec![session.screen.pid()];
        keep.extend(session.chrome.as_ref().map(chrome::Chrome::pid));
        keep
    })
    .unwrap_or_default();
    let Ok(entries) = std::fs::read_dir("/proc") else { return };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
        if pid == 1 || keep.contains(&pid) {
            continue;
        }
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
        let fields: Vec<&str> = stat.rsplit_once(')').map(|(_, rest)| rest.split_whitespace().collect()).unwrap_or_default();
        if fields.first() == Some(&"Z") && fields.get(1) == Some(&"1") {
            // SAFETY: a zombie child of ours.
            unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) };
        }
    }
}

/// factoryd's frames, read and sorted into events; the first that breaks
/// the wire (too long, unknown, the wrong way) ends it.
fn read_in(tell: Sender<Event>) {
    let mut stdin = std::io::stdin().lock();
    loop {
        let event = match wire::read(&mut stdin, Way::In) {
            Ok(None) => Event::Closed(None),
            Err(error) => Event::Closed(Some(error.to_string())),
            Ok(Some((Channel::Control, payload))) => wire::decode(&payload).map_or(Event::BadControl, Event::Control),
            Ok(Some((Channel::Cdp, payload))) => Event::Cdp(payload),
            Ok(Some((channel, payload))) => Event::Screen(channel, payload),
        };
        let closed = matches!(event, Event::Closed(_));
        if tell.send(event).is_err() || closed {
            return;
        }
    }
}

/// Every frame out, one at a time, on stdout.
fn write_out(outgoing: Receiver<(Channel, Vec<u8>)>, tell: Sender<Event>, out: Outbox) {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    for (channel, payload) in outgoing {
        out.sent(channel, payload.len());
        let mut frame = Vec::with_capacity(wire::HEADER + payload.len());
        if wire::write(&mut frame, channel, &payload, Way::Out).is_err() {
            continue;
        }
        if stdout.write_all(&frame).and_then(|()| stdout.flush()).is_err() {
            let _ = tell.send(Event::Closed(Some("stdout closed".to_owned())));
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, SocketAddr};

    /// v4 addresses are checked at the v4 listener and v6 at the v6 one,
    /// never a v4 one at a [::] that may take IPv6 only (Fugo 37547).
    #[test]
    fn the_firewall_check_tries_each_address_at_its_own_stacks_listener() {
        let own: Vec<IpAddr> = ["127.0.0.1", "10.0.0.5", "2001:db8::5", "fe80::1", "::1"].iter().map(|text| text.parse().unwrap()).collect();
        let at = |text: &str| text.parse::<SocketAddr>().unwrap();
        assert_eq!(
            super::targets(4000, Some(6000), own.clone()),
            vec![at("127.0.0.1:4000"), at("[::1]:6000"), at("10.0.0.5:4000"), at("[2001:db8::5]:6000")]
        );
        assert_eq!(super::targets(4000, None, own), vec![at("127.0.0.1:4000"), at("10.0.0.5:4000")]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_v6_listener_takes_ipv6_only_whatever_the_system_says() {
        use std::os::fd::AsRawFd;
        let Some(listener) = super::v6_only_listener() else { return };
        let mut on: libc::c_int = 0;
        let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: getsockopt into a c_int that outlives the call.
        let read = unsafe {
            libc::getsockopt(listener.as_raw_fd(), libc::IPPROTO_IPV6, libc::IPV6_V6ONLY, (&raw mut on).cast(), &mut length)
        };
        assert_eq!((read, on), (0, 1));
        assert!(listener.local_addr().unwrap().is_ipv6());
    }
}
