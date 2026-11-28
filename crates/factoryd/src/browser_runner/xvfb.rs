//! The browser's private screen: Xvfb, which only holders of its cookie can
//! connect to (agent-browser.md R1, Fugo 36536). Its socket is in the
//! runner's own /tmp/.X11-unix, a tmpfs only its mount namespace sees
//! (`namespace::own_x_sockets`), and the cookie, in a 0600 file in the
//! runner's 0700 folder, is the gate within: no TCP, and no abstract
//! socket (`-nolisten local`, which the abstract-unix scope of Landlock
//! would also stop Chrome from reaching; spike finding 4).

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use x11rb::rust_connection::{DefaultStream, RustConnection};

/// How long Xvfb has to say it's ready.
const READY: Duration = Duration::from_secs(10);

/// The cookie's protocol.
const COOKIE_NAME: &[u8] = b"MIT-MAGIC-COOKIE-1";

/// A running screen's address and cookie: what an X client of the runner's
/// (capture, input, the clipboard, closing Chrome's windows) connects with.
#[derive(Debug, Clone)]
pub struct Display {
    pub number: u32,
    /// The Xauthority file Chrome reads (XAUTHORITY).
    pub cookie_path: PathBuf,
    pub(super) cookie: [u8; 16],
}

impl Display {
    /// As DISPLAY spells it.
    pub fn name(&self) -> String {
        format!(":{}", self.number)
    }

    /// A connection to the screen, with its cookie (never XAUTHORITY or
    /// DISPLAY from the environment), screen 0.
    pub fn connect(&self) -> Result<RustConnection, String> {
        let socket = format!("/tmp/.X11-unix/X{}", self.number);
        let stream = std::os::unix::net::UnixStream::connect(socket)
            .and_then(DefaultStream::from_unix_stream)
            .map_err(|error| format!("couldn't reach the screen: {error}"))?
            .0;
        RustConnection::connect_to_stream_with_auth_info(stream, 0, COOKIE_NAME.to_vec(), self.cookie.to_vec())
            .map_err(|error| format!("the screen refused the runner: {error}"))
    }
}

/// The panel a desktop keeps at the bottom of its screen, as tall as KDE
/// Plasma's: the window manager leaves it out of the work area, so pages
/// read the screen less the panel (`screen.availHeight`) and a window
/// maximised to the rest, as on a desktop.
pub(crate) const PANEL: u16 = 44;

/// Xvfb, running, with its window manager when there is one.
pub struct Screen {
    pub display: Display,
    child: Child,
    window_manager: Option<Child>,
    /// Why it has no window manager though one is installed: said, and
    /// the browser runs on the whole screen.
    pub without_window_manager: Option<String>,
}

impl Screen {
    /// Starts a screen `width`×`height`, its cookie and log in `tmp` (the
    /// runner's 0700 folder). Xvfb picks a free display itself
    /// (`-displayfd`) and says it once it's ready.
    pub fn start(tmp: &Path, width: u16, height: u16) -> Result<Screen, String> {
        let mut cookie = [0u8; 16];
        getrandom::fill(&mut cookie).map_err(|error| format!("no random cookie: {error}"))?;
        let cookie_path = tmp.join("Xauthority");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&cookie_path)
            .map_err(|error| format!("couldn't write the screen's cookie: {error}"))?;
        file.write_all(&xauthority(&cookie)).map_err(|error| format!("couldn't write the screen's cookie: {error}"))?;
        drop(file);
        let log = File::create(tmp.join("xvfb.log")).map_err(|error| format!("couldn't make the screen's log: {error}"))?;

        let (ready_read, ready_write) = pipe()?;
        let ready_write = above_stdio(ready_write)?;
        let ready_fd = ready_write.as_raw_fd();
        let mut command = Command::new(xvfb_binary()?);
        command
            .args(["-displayfd", "3", "-auth"])
            .arg(&cookie_path)
            .args(["-nolisten", "tcp", "-nolisten", "local", "-screen", "0"])
            .arg(format!("{width}x{height}x24"))
            .env_clear()
            .env("PATH", super::chrome::PATH)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        // SAFETY: between fork and exec, only async-signal-safe calls.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(ready_fd, 3) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                super::die_with_parent();
                Ok(())
            });
        }
        let child = command.spawn().map_err(|error| format!("couldn't start the screen (Xvfb): {error}"))?;
        drop(ready_write);
        let mut screen =
            Screen { display: Display { number: 0, cookie_path, cookie }, child, window_manager: None, without_window_manager: None };
        match read_number(ready_read) {
            Ok(number) => {
                screen.display.number = number;
                match window_manager(&screen.display, tmp, height) {
                    Ok(window_manager) => screen.window_manager = window_manager,
                    Err(why) => screen.without_window_manager = Some(why),
                }
                Ok(screen)
            }
            Err(why) => {
                screen.stop();
                Err(why)
            }
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Stops it: asked, then made to.
    pub fn stop(&mut self) {
        if let Some(window_manager) = &mut self.window_manager {
            super::end(window_manager, Duration::from_secs(2));
        }
        super::end(&mut self.child, Duration::from_secs(2));
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            self.stop();
        }
    }
}

/// An Xauthority file with one cookie for every display on this machine
/// (family Wild, no number): Xvfb takes the cookies in the file it's
/// given whatever they're for, and Chrome's libxcb matches it to any.
fn xauthority(cookie: &[u8; 16]) -> Vec<u8> {
    let mut file = Vec::new();
    file.extend_from_slice(&0xffffu16.to_be_bytes());
    for field in [&b""[..], b"", COOKIE_NAME, cookie] {
        file.extend_from_slice(&(field.len() as u16).to_be_bytes());
        file.extend_from_slice(field);
    }
    file
}

/// The display number Xvfb writes to its -displayfd once it's ready.
fn read_number(ready: OwnedFd) -> Result<u32, String> {
    let deadline = Instant::now() + READY;
    let mut said = Vec::new();
    let mut file = File::from(ready);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("the screen (Xvfb) didn't start in time".to_owned());
        }
        let mut poll = libc::pollfd { fd: file.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd, ours.
        let ready = unsafe { libc::poll(&mut poll, 1, left.as_millis().min(i32::MAX as u128) as i32) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("couldn't wait for the screen: {error}"));
        }
        if ready == 0 {
            continue;
        }
        let mut chunk = [0u8; 16];
        let read = file.read(&mut chunk).map_err(|error| format!("couldn't hear from the screen: {error}"))?;
        if read == 0 {
            return Err("the screen (Xvfb) ended as it started: its log is in the runner's folder".to_owned());
        }
        said.extend_from_slice(&chunk[..read]);
        if said.contains(&b'\n') || said.len() > 8 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&said);
    text.trim().parse().map_err(|_| "the screen (Xvfb) gave no display number".to_owned())
}

/// Openbox's settings: one desktop and the panel's margin. Openbox runs
/// outside Chrome's fence, so nothing in it may start a program: no key or
/// mouse bindings, and its only menu `menu`, which is empty.
/// scripts/browser-image/test-window-manager.sh checks that in the image,
/// with these settings copied: change both together.
fn openbox_config(menu: &Path, theme: Option<&Path>) -> String {
    let theme = theme.map(|theme| format!("\n  <theme><name>{}</name></theme>", xml_text(&theme.to_string_lossy()))).unwrap_or_default();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<openbox_config xmlns="http://openbox.org/3.4/rc">
  <margins><top>0</top><bottom>{PANEL}</bottom><left>0</left><right>0</right></margins>
  <desktops><number>1</number></desktops>{theme}
  <keyboard></keyboard>
  <mouse></mouse>
  <menu><file>{menu}</file></menu>
</openbox_config>
"#,
        menu = xml_text(&menu.to_string_lossy()),
    )
}

/// Openbox's theme, by its path: it won't start without one, and its own
/// XDG folders (`window_manager`) have none. Ubuntu's, then NixOS's.
const THEMES: [&str; 2] = ["/usr/share/themes/Clearlooks", "/run/current-system/sw/share/themes/Clearlooks"];

/// The empty menu Openbox is given instead of the system's, whose items
/// start programs.
const OPENBOX_MENU: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<openbox_menu xmlns=\"http://openbox.org/3.4/menu\"></openbox_menu>\n";

/// `text` with XML's special characters escaped.
fn xml_text(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Openbox on the screen, when it's installed: a desktop's window manager,
/// so the work area stops above the panel and Chrome's window is maximised
/// to it. Ready once it says so on the screen's root (`_NET_WORKAREA`).
/// None without Openbox; Err when it's there but didn't start (the browser
/// then runs on the whole screen, as without it).
fn window_manager(display: &Display, tmp: &Path, height: u16) -> Result<Option<Child>, String> {
    let Some(openbox) = super::chrome::find(&["openbox"]) else { return Ok(None) };
    // Its own folders for everything XDG, so it reads nothing of the
    // system's (/etc/xdg/openbox: its menu, its autostart).
    let xdg = tmp.join("openbox");
    for folder in ["config", "data"] {
        std::fs::create_dir_all(xdg.join(folder)).map_err(|error| format!("couldn't make the window manager's folder: {error}"))?;
    }
    let menu = xdg.join("menu.xml");
    let config = xdg.join("openbox.xml");
    std::fs::write(&menu, OPENBOX_MENU).map_err(|error| format!("couldn't write the window manager's menu: {error}"))?;
    let theme = THEMES.iter().map(Path::new).find(|theme| theme.is_dir());
    std::fs::write(&config, openbox_config(&menu, theme)).map_err(|error| format!("couldn't write the window manager's settings: {error}"))?;
    let log = File::create(tmp.join("openbox.log")).map_err(|error| format!("couldn't make the window manager's log: {error}"))?;
    let mut command = Command::new(openbox);
    command
        .arg("--sm-disable")
        .arg("--config-file")
        .arg(&config)
        .env_clear()
        .env("PATH", super::chrome::PATH)
        .env("HOME", tmp)
        .env("XDG_CONFIG_HOME", xdg.join("config"))
        .env("XDG_CONFIG_DIRS", xdg.join("config"))
        .env("XDG_DATA_HOME", xdg.join("data"))
        .env("XDG_DATA_DIRS", xdg.join("data"))
        .env("DISPLAY", display.name())
        .env("XAUTHORITY", &display.cookie_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    // SAFETY: between fork and exec, only async-signal-safe calls.
    unsafe {
        command.pre_exec(|| {
            super::die_with_parent();
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|error| format!("couldn't start the window manager (Openbox): {error}"))?;
    let deadline = Instant::now() + READY;
    let usable = height.saturating_sub(PANEL);
    loop {
        if work_area_height(display).is_some_and(|height| height == u32::from(usable)) {
            return Ok(Some(child));
        }
        if !matches!(child.try_wait(), Ok(None)) || Instant::now() > deadline {
            super::end(&mut child, Duration::from_secs(2));
            return Err("the window manager (Openbox) didn't start, so the browser has the whole screen".to_owned());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The height of the screen's work area, as its window manager sets it on
/// the root (`_NET_WORKAREA`: x, y, width, height for each desktop).
fn work_area_height(display: &Display) -> Option<u32> {
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};
    let connection = display.connect().ok()?;
    let root = connection.setup().roots.first()?.root;
    let atom = connection.intern_atom(false, b"_NET_WORKAREA").ok()?.reply().ok()?.atom;
    let said = connection.get_property(false, root, atom, AtomEnum::CARDINAL, 0, 4).ok()?.reply().ok()?;
    said.value32()?.nth(3)
}

fn xvfb_binary() -> Result<PathBuf, String> {
    super::chrome::find(&["Xvfb"]).ok_or_else(|| "Xvfb isn't installed: run Claktory's browser setup".to_owned())
}

/// A pipe, both ends close-on-exec.
pub(crate) fn pipe() -> Result<(OwnedFd, OwnedFd), String> {
    let mut fds = [0; 2];
    #[cfg(target_os = "linux")]
    // SAFETY: two ints for the kernel to fill.
    let made = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    // macOS has no pipe2: the Mac only builds this, it never runs it.
    #[cfg(not(target_os = "linux"))]
    // SAFETY: two ints for the kernel to fill, then flags on them.
    let made = unsafe {
        let made = libc::pipe(fds.as_mut_ptr());
        if made == 0 {
            libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fds[1], libc::F_SETFD, libc::FD_CLOEXEC);
        }
        made
    };
    if made != 0 {
        return Err(format!("no pipe: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: both just made, and only ours.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

/// `fd` moved to a number of 64 or more, close-on-exec, so a child's
/// dup2 onto 3 or 4 never lands on another of the fds it's given.
pub(crate) fn above_stdio(fd: OwnedFd) -> Result<OwnedFd, String> {
    // SAFETY: a new fd, ours.
    let moved = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 64) };
    if moved < 0 {
        return Err(format!("couldn't move a descriptor: {}", std::io::Error::last_os_error()));
    }
    // SAFETY: just made, ours.
    Ok(unsafe { OwnedFd::from_raw_fd(moved) })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing in Openbox's settings can start a program: bindings empty,
    /// and the menu it reads only the empty one given, its path escaped.
    #[test]
    fn openbox_is_given_no_bindings_and_only_an_empty_menu() {
        let config = openbox_config(Path::new("/tmp/claktory-<1>&/openbox/menu.xml"), Some(Path::new("/usr/share/themes/Clearlooks")));
        assert!(config.contains("<theme><name>/usr/share/themes/Clearlooks</name></theme>"), "{config}");
        assert!(!openbox_config(Path::new("/tmp/menu.xml"), None).contains("<theme>"));
        assert!(config.contains("<keyboard></keyboard>") && config.contains("<mouse></mouse>"), "{config}");
        assert!(config.contains("<menu><file>/tmp/claktory-&lt;1&gt;&amp;/openbox/menu.xml</file></menu>"), "{config}");
        assert_eq!(config.matches("<file>").count(), 1, "{config}");
        assert!(!config.contains("<command") && !config.contains("<action"), "{config}");
        assert!(!OPENBOX_MENU.contains("<item") && !OPENBOX_MENU.contains("<command"), "{OPENBOX_MENU}");
    }

    #[test]
    fn the_cookie_file_holds_one_wild_cookie() {
        let cookie = [7u8; 16];
        let file = xauthority(&cookie);
        // family, address, number, name, data
        let mut expected = vec![0xff, 0xff, 0, 0, 0, 0, 0, 18];
        expected.extend_from_slice(b"MIT-MAGIC-COOKIE-1");
        expected.extend_from_slice(&[0, 16]);
        expected.extend_from_slice(&cookie);
        assert_eq!(file, expected);
    }
}
