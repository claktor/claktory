//! Claktory's Chrome on the private screen (agent-browser.md R1, §3.1, §5.1):
//! branded Google Chrome on the kept profile, driven over its DevTools pipe
//! (fds 3 and 4, never a port) or undriven, with no pipe at all, for the
//! person's turn. Its argument list is fixed here; the only thing from
//! outside is the page to open, which must be an http or https address and
//! goes after `--`, so it can never read as a flag (Fugo 36597).

use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use super::Event;
use super::wire::{MAX_CDP, Mode};
use super::xvfb::{Display, above_stdio, pipe};

/// The PATH the screen and Chrome run with.
pub(crate) const PATH: &str = "/usr/bin:/bin:/run/current-system/sw/bin";

/// How long Chrome has to quit when asked as a person would, then when
/// told to (SIGTERM), before it's killed.
const QUIT: Duration = Duration::from_secs(15);
const TERMINATED: Duration = Duration::from_secs(10);

/// The message that closes a driven Chrome, as its Quit does: its id the
/// highest, never one factoryd's own messages use.
const CLOSE: &[u8] = br#"{"id":2147483647,"method":"Browser.close"}"#;

/// Flags that must never reach Chrome: a debugging port, remote origins,
/// extensions loaded from a folder, or no sandbox.
const NEVER: [&str; 6] = [
    "--remote-debugging-port",
    "--remote-debugging-address",
    "--remote-allow-origins",
    "--load-extension",
    "--enable-unsafe-extension-debugging",
    "--no-sandbox",
];

/// The first of `names` on PATH, or at an absolute path.
pub(crate) fn find(names: &[&str]) -> Option<PathBuf> {
    for name in names {
        if name.starts_with('/') {
            if Path::new(name).is_file() {
                return Some(PathBuf::from(name));
            }
            continue;
        }
        for folder in PATH.split(':') {
            let path = Path::new(folder).join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Google Chrome where it's installed: Google's package first, then the
/// names it goes by on PATH (NixOS's), then Chromium.
pub(crate) fn binary() -> Result<PathBuf, String> {
    let found = find(&["/opt/google/chrome/chrome", "google-chrome-stable", "google-chrome", "chromium"])
        .ok_or_else(|| "Google Chrome isn't installed: run Claktory's browser setup".to_owned())?;
    std::fs::canonicalize(&found).map_err(|error| format!("couldn't find Chrome: {error}"))
}

/// Chrome's version, as `--version` says it ("158.0.8127.88").
pub(crate) fn version(binary: &Path) -> String {
    let said = Command::new(binary).arg("--version").env_clear().env("PATH", PATH).stdin(Stdio::null()).stderr(Stdio::null()).output();
    let text = said.map(|output| String::from_utf8_lossy(&output.stdout).into_owned()).unwrap_or_default();
    text.split_whitespace().find(|word| word.chars().next().is_some_and(|first| first.is_ascii_digit())).unwrap_or("").chars().take(32).collect()
}

/// The page to open: an http or https address, as the URL parser spells
/// it; anything else is refused.
pub(crate) fn checked_url(url: &str) -> Result<String, String> {
    let parsed = url::Url::parse(url).map_err(|_| "the page to open isn't a web address".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("the page to open must be an http or https address".to_owned());
    }
    Ok(parsed.to_string())
}

/// The person's time zone and languages, checked (agent-browser-checks.md
/// change 3): what their own browser would say, not a server's UTC and
/// en-US.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Person {
    /// An IANA name this computer has the data for.
    pub time_zone: String,
    /// The folder its data is in, when it isn't glibc's own
    /// (/usr/share/zoneinfo): NixOS's /etc/zoneinfo, for TZDIR.
    pub zone_folder: Option<PathBuf>,
    /// BCP 47 tags, the preferred first; never empty.
    pub languages: Vec<String>,
}

impl Default for Person {
    fn default() -> Person {
        Person { time_zone: "UTC".to_owned(), zone_folder: None, languages: vec!["en-US".to_owned(), "en".to_owned()] }
    }
}

/// Where the time zones' data is: Debian's and Ubuntu's, then NixOS's.
const ZONE_FOLDERS: [&str; 2] = ["/usr/share/zoneinfo", "/etc/zoneinfo"];

/// The folder holding `zone`'s data when it's a time zone's name (as
/// `wire::zone_name_ok` says) that this computer has a file for, inside
/// that folder once every link is followed (Fugo 37383): Some(None) for
/// glibc's own folder, Some(Some(folder)) for another; None otherwise.
pub(crate) fn zone_folder(zone: &str) -> Option<Option<PathBuf>> {
    if !super::wire::zone_name_ok(zone) {
        return None;
    }
    ZONE_FOLDERS.iter().enumerate().find_map(|(at, folder)| {
        let real = std::fs::canonicalize(folder).ok()?;
        let file = std::fs::canonicalize(Path::new(folder).join(zone)).ok()?;
        (file.starts_with(&real) && file.is_file()).then(|| (at > 0).then(|| PathBuf::from(folder)))
    })
}

/// Chrome's LANG: the person's first language as a POSIX locale
/// ("en-GB" → en_GB.UTF-8, "fr" → fr.UTF-8), which ICU, and so the page's
/// Intl, takes as its locale (with C.UTF-8 a page's dates would read en-US
/// whatever navigator.language says). Built only from a tag
/// `wire::language_ok` passed. glibc itself falls back to C where the
/// locale isn't generated, which Chrome doesn't need.
pub(crate) fn lang(person: &Person) -> String {
    let first = person.languages.first().map(String::as_str).unwrap_or("en-US");
    let mut parts = first.split('-');
    let language = parts.next().unwrap_or_default().to_ascii_lowercase();
    match parts.find(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic())) {
        Some(region) => format!("{language}_{}.UTF-8", region.to_ascii_uppercase()),
        None => format!("{language}.UTF-8"),
    }
}

/// Writes into the profile's Preferences before each launch, through
/// serde_json, never by joining text into the file (Fugo 37383):
/// - the person's languages (`intl.accept_languages` and
///   `intl.selected_languages`), what pages see in navigator.languages and
///   Accept-Language;
/// - that Chrome exited cleanly (`profile.exit_type`), so a Chrome that was
///   killed (its runner ended, the computer restarted) restores its tabs
///   as `restore_on_startup` says rather than opening a new tab with
///   "Restore pages?", which left a handoff on the wrong page (the
///   end-to-end run on the box, 26 Nov).
///
/// The rest stays as Chrome left it.
pub(crate) fn prepare_preferences(profile: &Path, person: &Person) -> std::io::Result<()> {
    let path = profile.join("Default").join("Preferences");
    let mut preferences: serde_json::Value = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::json!({})),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error),
    };
    if !preferences.is_object() {
        preferences = serde_json::json!({});
    }
    let intl = preferences.as_object_mut().map(|all| all.entry("intl").or_insert_with(|| serde_json::json!({})));
    if let Some(intl) = intl {
        if !intl.is_object() {
            *intl = serde_json::json!({});
        }
        // Chrome writes selected_languages beside accept_languages once it has
        // run, and reads it first, so both say the person's languages.
        let languages = serde_json::Value::String(person.languages.join(","));
        intl["accept_languages"] = languages.clone();
        intl["selected_languages"] = languages;
    }
    let chrome_profile = preferences.as_object_mut().map(|all| all.entry("profile").or_insert_with(|| serde_json::json!({})));
    if let Some(chrome_profile) = chrome_profile {
        if !chrome_profile.is_object() {
            *chrome_profile = serde_json::json!({});
        }
        chrome_profile["exit_type"] = serde_json::Value::String("Normal".to_owned());
        chrome_profile["exited_cleanly"] = serde_json::Value::Bool(true);
    }
    // Private, as Chrome keeps it, and whole or not at all.
    let written = path.with_extension("claktory");
    let _ = std::fs::remove_file(&written);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&written)?;
    file.write_all(preferences.to_string().as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&written, &path)
}

/// What a launch needs.
pub(crate) struct Launch<'a> {
    pub binary: &'a Path,
    /// Chrome's user data folder, inside the kept profile.
    pub profile: &'a Path,
    /// The profile's HOME, for fontconfig's cache and the like.
    pub home: &'a Path,
    /// The runner's private folder (TMPDIR, the screen's cookie).
    pub tmp: &'a Path,
    pub display: &'a Display,
    pub person: &'a Person,
    pub mode: Mode,
    pub url: Option<&'a str>,
    pub width: u16,
    pub height: u16,
    /// Landlock's rules, made beforehand: the child only applies them.
    pub fence: Option<&'a OwnedFd>,
}

/// Chrome's arguments, all of them: fixed but for the window's size, the
/// profile, the pipe when driven, and the page after `--`.
pub(crate) fn arguments(launch: &Launch<'_>) -> Result<Vec<OsString>, String> {
    let mut profile = OsString::from("--user-data-dir=");
    profile.push(launch.profile);
    let mut arguments: Vec<OsString> = vec![
        profile,
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-sync".into(),
        "--password-store=basic".into(),
        format!("--lang={}", launch.person.languages.first().map(String::as_str).unwrap_or("en-US")).into(),
        "--window-position=0,0".into(),
        format!("--window-size={},{}", launch.width, launch.height).into(),
        "--start-maximized".into(),
        // WebGL through SwiftShader, on the CPU: Chrome reaches no GPU here
        // (a server has none, and Landlock leaves out /dev/dri), and Chrome
        // (158 on the spike box) falls back to SwiftShader only when told. A
        // browser with no WebGL is a bot tell (spike box: sannysoft, CreepJS,
        // webscraper.io). Unsafe because SwiftShader compiles pages' shaders
        // in the GPU process, which is still sandboxed and fenced as all of
        // Chrome is; it holds while Chrome is rebuilt within a week of each
        // security release (Fugo 37222, Bruno 37225). The Mac image's VM
        // runs this runner too, so it gets it as well: P6's first run checks
        // that the VM has no WebGL without it, or it becomes the host's
        // choice (Fugo 37292).
        "--enable-unsafe-swiftshader".into(),
    ];
    if launch.mode == Mode::Driven {
        arguments.push("--remote-debugging-pipe".into());
    }
    if let Some(url) = launch.url {
        arguments.push("--".into());
        arguments.push(checked_url(url)?.into());
    }
    if let Some(never) = arguments.iter().find(|argument| NEVER.iter().any(|flag| argument.to_string_lossy().starts_with(flag))) {
        return Err(format!("refused a flag Chrome must never get: {}", never.to_string_lossy()));
    }
    Ok(arguments)
}

/// The Preferences a profile starts with, before its first launch only (a
/// kept profile keeps its own): sessions restored, no password manager,
/// autofill or sign-in to Chrome, downloads asked, notifications, camera,
/// microphone and location refused, and a colour of its own.
pub(crate) fn first_preferences(profile: &Path) -> std::io::Result<bool> {
    let default = profile.join("Default");
    let path = default.join("Preferences");
    if path.exists() {
        return Ok(false);
    }
    super::private_folder(&default)?;
    let preferences = serde_json::json!({
        "session": {"restore_on_startup": 1},
        "credentials_enable_service": false,
        "profile": {
            "password_manager_enabled": false,
            "default_content_setting_values": {"notifications": 2, "geolocation": 2, "media_stream_camera": 2, "media_stream_mic": 2},
        },
        "autofill": {"profile_enabled": false, "credit_card_enabled": false},
        "signin": {"allowed": false},
        "download": {"prompt_for_download": true},
        "browser": {"has_seen_welcome_page": true, "theme": {"user_color": -12434878}},
    });
    std::fs::write(&path, preferences.to_string())?;
    Ok(true)
}

/// A Chrome that died (or ran in the VM before this one, with another
/// hostname) leaves its Singleton files, and Chrome then refuses the
/// profile as in use on another computer: the runner, one Chrome per
/// profile, clears them (spike finding 5).
fn clear_singletons(profile: &Path) {
    for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let _ = std::fs::remove_file(profile.join(name));
    }
}

/// Chrome, running.
pub(crate) struct Chrome {
    pub mode: Mode,
    pid: u32,
    /// The DevTools pipe's writing end; None undriven, where there's none.
    to_chrome: Option<File>,
    /// Its exit, once: the watcher's.
    exited: Receiver<Option<i32>>,
    done: bool,
}

impl Chrome {
    /// Starts Chrome, reporting what it says on its pipe and its end as
    /// `events` tagged `generation`.
    pub fn launch(launch: &Launch<'_>, events: Sender<Event>, generation: u64) -> Result<Chrome, String> {
        let arguments = arguments(launch)?;
        clear_singletons(launch.profile);
        prepare_preferences(launch.profile, launch.person).map_err(|error| format!("couldn't prepare the browser's settings: {error}"))?;
        let log = File::create(launch.tmp.join("chrome.log")).map_err(|error| format!("couldn't make Chrome's log: {error}"))?;
        let log_too = log.try_clone().map_err(|error| format!("couldn't make Chrome's log: {error}"))?;
        let mut command = Command::new(launch.binary);
        command
            .args(&arguments)
            .env_clear()
            .env("DISPLAY", launch.display.name())
            .env("XAUTHORITY", &launch.display.cookie_path)
            .env("HOME", launch.home)
            .env("TMPDIR", launch.tmp)
            .env("PATH", PATH)
            .env("LANG", lang(launch.person))
            .env("TZ", &launch.person.time_zone)
            .stdin(Stdio::null())
            .stdout(log)
            .stderr(log_too);
        if let Some(folder) = &launch.person.zone_folder {
            command.env("TZDIR", folder);
        }
        // Driven: two pipes, Chrome's ends at 3 (it reads) and 4 (it
        // writes). Undriven: none, so nothing can drive it (Fugo 36536).
        let pipes = match launch.mode {
            Mode::Driven => {
                let (chrome_reads, we_write) = pipe()?;
                let (we_read, chrome_writes) = pipe()?;
                Some((above_stdio(chrome_reads)?, we_write, we_read, above_stdio(chrome_writes)?))
            }
            Mode::Undriven => None,
        };
        let ends = pipes.as_ref().map(|(reads, _, _, writes)| (reads.as_raw_fd(), writes.as_raw_fd()));
        let fence = launch.fence.map(AsRawFd::as_raw_fd);
        // SAFETY: between fork and exec, only async-signal-safe calls.
        unsafe {
            command.pre_exec(move || {
                if let Some((reads, writes)) = ends
                    && (libc::dup2(reads, 3) < 0 || libc::dup2(writes, 4) < 0)
                {
                    return Err(std::io::Error::last_os_error());
                }
                // Its own session, so the runner's signals stay the runner's.
                libc::setsid();
                super::die_with_parent();
                if let Some(fence) = fence {
                    super::fence_self(fence)?;
                }
                Ok(())
            });
        }
        let child = command.spawn().map_err(|error| format!("couldn't start Chrome: {error}"))?;
        let pid = child.id();
        let to_chrome = pipes.map(|(chrome_reads, we_write, we_read, chrome_writes)| {
            drop((chrome_reads, chrome_writes));
            let events = events.clone();
            std::thread::spawn(move || read_devtools(File::from(we_read), events, generation));
            File::from(we_write)
        });
        let (exit, exited) = mpsc::channel();
        std::thread::spawn(move || watch(child, exit, events, generation));
        Ok(Chrome { mode: launch.mode, pid, to_chrome, exited, done: false })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// One DevTools message to Chrome; Err when it's undriven or gone.
    pub fn send(&mut self, message: &[u8]) -> Result<(), String> {
        let Some(to_chrome) = &mut self.to_chrome else {
            return Err("Chrome isn't driven now".to_owned());
        };
        if message.contains(&0) {
            return Err("a DevTools message can't hold a NUL".to_owned());
        }
        to_chrome.write_all(message).and_then(|()| to_chrome.write_all(b"\0")).map_err(|error| format!("Chrome's pipe: {error}"))
    }

    /// Whether it ended (its watcher said so).
    pub fn ended(&mut self) -> bool {
        if !self.done && self.exited.try_recv().is_ok() {
            self.done = true;
        }
        self.done
    }

    /// Quits Chrome as a person would, so it writes its cookies first
    /// (SIGTERM loses the last 30 s of them; spike finding 8): driven, its
    /// Browser.close; undriven, each window asked to close, as its close
    /// button does, since there's no window manager to give a Quit
    /// shortcut focus. Then SIGTERM, then SIGKILL. Says which did it.
    pub fn quit(&mut self, display: &Display) -> &'static str {
        if self.ended() {
            return "ended";
        }
        if self.to_chrome.is_some() {
            if self.send(CLOSE).is_ok() && self.wait(QUIT) {
                return "quit";
            }
        } else {
            // Asked again each second: a window that wasn't up yet (Chrome
            // just started) or a second one opened meanwhile closes too.
            let deadline = std::time::Instant::now() + QUIT;
            while std::time::Instant::now() < deadline {
                let _ = close_windows(display);
                if self.wait(Duration::from_secs(1)) {
                    return "quit";
                }
            }
        }
        self.signal(libc::SIGTERM);
        if self.wait(TERMINATED) {
            return "terminated";
        }
        self.signal(libc::SIGKILL);
        self.wait(Duration::from_secs(5));
        "killed"
    }

    fn wait(&mut self, within: Duration) -> bool {
        if self.done {
            return true;
        }
        match self.exited.recv_timeout(within) {
            Ok(_) | Err(RecvTimeoutError::Disconnected) => {
                self.done = true;
                true
            }
            Err(RecvTimeoutError::Timeout) => false,
        }
    }

    /// Chrome's whole session (its own process group), while it runs.
    fn signal(&mut self, signal: i32) {
        if !self.ended() {
            // SAFETY: a signal to the group Chrome leads, not yet reaped.
            unsafe { libc::kill(-(self.pid as i32), signal) };
        }
    }
}

/// Chrome's DevTools messages, NUL-separated, to the runner's events, each
/// at most MAX_CDP: a longer one is dropped whole, never cut.
fn read_devtools(mut from_chrome: File, events: Sender<Event>, generation: u64) {
    let mut pending: Vec<u8> = Vec::new();
    let mut dropping = false;
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        let read = match from_chrome.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        for piece in chunk[..read].split_inclusive(|byte| *byte == 0) {
            let whole = piece.last() == Some(&0);
            let body = if whole { &piece[..piece.len() - 1] } else { piece };
            if !dropping {
                if pending.len() + body.len() > MAX_CDP {
                    pending.clear();
                    dropping = true;
                    let _ = events.send(Event::Oversized(generation));
                } else {
                    pending.extend_from_slice(body);
                }
            }
            if whole {
                if !dropping && events.send(Event::FromChrome(generation, std::mem::take(&mut pending))).is_err() {
                    return;
                }
                pending.clear();
                dropping = false;
            }
        }
    }
}

fn watch(mut child: Child, exit: Sender<Option<i32>>, events: Sender<Event>, generation: u64) {
    let code = child.wait().ok().and_then(|status| status.code());
    let _ = exit.send(code);
    let _ = events.send(Event::ChromeExited(generation, code));
}

/// Asks each top-level window to close (WM_DELETE_WINDOW), as its close
/// button would.
fn close_windows(display: &Display) -> Result<(), String> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask};
    let connection = display.connect()?;
    let fail = |error: &dyn std::fmt::Display| format!("couldn't ask Chrome's windows to close: {error}");
    let root = connection.setup().roots[0].root;
    let atom = |name: &[u8]| -> Result<u32, String> {
        Ok(connection.intern_atom(false, name).map_err(|e| fail(&e))?.reply().map_err(|e| fail(&e))?.atom)
    };
    let (protocols, delete) = (atom(b"WM_PROTOCOLS")?, atom(b"WM_DELETE_WINDOW")?);
    let windows = connection.query_tree(root).map_err(|e| fail(&e))?.reply().map_err(|e| fail(&e))?.children;
    for window in windows {
        let Ok(said) = connection.get_property(false, window, protocols, AtomEnum::ATOM, 0, 32).map_err(|e| fail(&e))?.reply() else {
            continue;
        };
        if said.value32().is_some_and(|mut atoms| atoms.any(|atom| atom == delete)) {
            let event = ClientMessageEvent::new(32, window, protocols, [delete, x11rb::CURRENT_TIME, 0, 0, 0]);
            connection.send_event(false, window, EventMask::NO_EVENT, event).map_err(|e| fail(&e))?;
        }
    }
    connection.flush().map_err(|e| fail(&e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch<'a>(display: &'a Display, mode: Mode, url: Option<&'a str>) -> Launch<'a> {
        Launch {
            binary: Path::new("/opt/google/chrome/chrome"),
            profile: Path::new("/var/lib/claktory-browser/profiles/u1000-claktory/chrome"),
            home: Path::new("/var/lib/claktory-browser/profiles/u1000-claktory/home"),
            tmp: Path::new("/tmp/claktory-browser-x"),
            display,
            person: Box::leak(Box::new(Person { languages: vec!["en-GB".into(), "en".into()], ..Person::default() })),
            mode,
            url,
            width: 1280,
            height: 800,
            fence: None,
        }
    }

    #[test]
    fn the_browser_takes_the_persons_zone_and_languages_and_nothing_else() {
        let display = Display { number: 10, cookie_path: PathBuf::from("/tmp/x/Xauthority"), cookie: [0; 16] };
        let arguments: Vec<String> =
            arguments(&launch(&display, Mode::Driven, None)).unwrap().iter().map(|argument| argument.to_string_lossy().into_owned()).collect();
        assert!(arguments.contains(&"--lang=en-GB".to_owned()) && !arguments.iter().any(|argument| argument == "--lang=en-US"));

        let with = |languages: &[&str]| Person { languages: languages.iter().map(|tag| (*tag).to_owned()).collect(), ..Person::default() };
        assert_eq!(lang(&Person::default()), "en_US.UTF-8");
        assert_eq!(lang(&with(&["de-DE", "de"])), "de_DE.UTF-8");
        assert_eq!(lang(&with(&["zh-Hant-TW"])), "zh_TW.UTF-8");
        assert_eq!(lang(&with(&["fr"])), "fr.UTF-8");

        // Never a path, wherever links lead.
        for zone in ["../../etc/passwd", "/etc/passwd", ":/etc/passwd", "Europe/../../../etc/passwd", "No/Such_Zone"] {
            assert_eq!(zone_folder(zone), None, "{zone}");
        }
        if Path::new("/usr/share/zoneinfo/UTC").exists() {
            assert_eq!(zone_folder("UTC"), Some(None));
        }

        // Written into Preferences as JSON, keeping what Chrome put there.
        let dir = std::env::temp_dir().join(format!("claktory-languages-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Default")).unwrap();
        std::fs::write(dir.join("Default/Preferences"), r#"{"session":{"restore_on_startup":1},"intl":{"selected_languages":"x","spellcheck":"y"},"profile":{"exit_type":"Crashed","name":"Person 1"}}"#).unwrap();
        prepare_preferences(&dir, &with(&["de-DE", "de"])).unwrap();
        let written: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("Default/Preferences")).unwrap()).unwrap();
        assert_eq!((written["intl"]["accept_languages"].as_str(), written["intl"]["selected_languages"].as_str()), (Some("de-DE,de"), Some("de-DE,de")));
        assert_eq!((written["session"]["restore_on_startup"].as_i64(), written["intl"]["spellcheck"].as_str()), (Some(1), Some("y")));
        // A Chrome that was killed restores its tabs all the same.
        assert_eq!((written["profile"]["exit_type"].as_str(), written["profile"]["name"].as_str()), (Some("Normal"), Some("Person 1")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn chrome_never_gets_a_port_an_extension_or_no_sandbox_and_the_page_is_never_a_flag() {
        let display = Display { number: 10, cookie_path: PathBuf::from("/tmp/x/Xauthority"), cookie: [0; 16] };
        let driven = arguments(&launch(&display, Mode::Driven, Some("https://play.google.com/console"))).unwrap();
        let driven: Vec<String> = driven.iter().map(|argument| argument.to_string_lossy().into_owned()).collect();
        assert!(driven.contains(&"--remote-debugging-pipe".to_owned()) && driven.contains(&"--enable-unsafe-swiftshader".to_owned()));
        assert_eq!(&driven[driven.len() - 2..], ["--", "https://play.google.com/console"]);
        let undriven = arguments(&launch(&display, Mode::Undriven, None)).unwrap();
        assert!(!undriven.iter().any(|argument| argument.to_string_lossy().contains("remote-debugging")));
        for every in [&driven] {
            for flag in NEVER {
                assert!(!every.iter().any(|argument| argument.starts_with(flag)), "{flag}");
            }
        }
        for url in [
            "--remote-debugging-port=9222",
            "--remote-allow-origins=*",
            "--load-extension=/tmp/x",
            "-x",
            "file:///etc/passwd",
            "chrome://settings",
            "javascript:alert(1)",
            "data:text/html,hi",
            "",
        ] {
            assert!(arguments(&launch(&display, Mode::Undriven, Some(url))).is_err(), "{url}");
        }
        assert_eq!(checked_url("HTTPS://Example.com").unwrap(), "https://example.com/");
    }
}
