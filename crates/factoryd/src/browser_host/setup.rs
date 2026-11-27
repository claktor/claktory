//! What Claktory's browser needs on this computer (#435 P4, C4): checked
//! before it starts, and shown on the setup card with what fixes it. On
//! Linux: a supported system, Landlock ABI 4 or later, Chrome and Xvfb,
//! the browser's own user and folders, the sudo line for exactly the
//! runner, the empty folder the runner mounts Chrome's policy over (no
//! local network, no sign-in to Chrome itself: only in its own namespace,
//! never the person's Chrome) and the firewall's unit (`scripts/browser-setup-linux.md`, Pericolo's
//! table). Without them
//! Claktory refuses to run it, rather than run Chrome less contained. On a
//! Mac: Apple silicon, macOS 26 or later and Apple's container tool,
//! running (`mac`); or, asked for, the dev stand-in over this Mac's Chrome.
//!
//! Checks are read at most every 30 seconds (the Workspace asks often),
//! and again right before a start.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use factory_protocol::v1;

use crate::factory::Factory;

/// How long a check's answer is kept.
const KEPT: Duration = Duration::from_secs(30);

/// The paths setup installs (C4).
pub(crate) const USER: &str = "claktory-browser";
pub(crate) const HOME: &str = "/var/lib/claktory-browser";
pub(crate) const RUNNER_DIR: &str = "/opt/claktory-browser";
pub(crate) const RUNNER: &str = "/opt/claktory-browser/runner";
/// Where Google Chrome and Chromium read managed policy: the runner mounts
/// a tmpfs of its own over them and writes Claktory's policy there, so only
/// its Chrome reads it (Fugo 49031). Setup only makes them, empty and
/// root's, and takes out the policy file older setups wrote in them, which
/// the person's own Chrome read too.
const POLICY_FOLDERS: [&str; 2] = ["/etc/opt/chrome/policies/managed", "/etc/chromium/policies/managed"];
const OLD_POLICY: &str = "claktory.json";
const FIREWALL: &str = "/etc/claktory-browser.nft";
const FIREWALL_UNIT: &str = "/etc/systemd/system/claktory-browser-nft.service";
/// What `sudo -n -l` lists for the runner when the sudo line is there.
const SUDO_LINE: &str = "(claktory-browser) NOPASSWD: /opt/claktory-browser/runner browser-runner *";

/// The Mac's own Chrome, which the dev stand-in drives.
pub(crate) const MAC_CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

static CHECKED: Mutex<Option<(Instant, Option<v1::AgentBrowserSetup>)>> = Mutex::new(None);
static CHECKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Why it can't run here, with what fixes it; None when it can. Read now
/// (it may run `sudo -n -l`): never under the Factory's lock.
pub(crate) fn problem(factory: &Factory) -> Option<v1::AgentBrowserSetup> {
    let answer = check(factory.home().root());
    *CHECKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((Instant::now(), answer.clone()));
    answer
}

/// The last answer, for the Workspace (under the Factory's lock): read
/// again in the background once it's 30 seconds old.
pub(crate) fn cached(factory: &Factory) -> Option<v1::AgentBrowserSetup> {
    let checked = CHECKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let stale = checked.as_ref().is_none_or(|(at, _)| at.elapsed() >= KEPT);
    if stale && !CHECKING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        let home = factory.home().root().to_path_buf();
        std::thread::spawn(move || {
            let answer = check(&home);
            *CHECKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((Instant::now(), answer));
            CHECKING.store(false, std::sync::atomic::Ordering::Release);
        });
    }
    checked.and_then(|(_, answer)| answer)
}

fn check(home: &Path) -> Option<v1::AgentBrowserSetup> {
    if cfg!(target_os = "linux") {
        linux(home)
    } else if cfg!(target_os = "macos") && super::standin_asked() {
        (!Path::new(MAC_CHROME).is_file()).then(|| v1::AgentBrowserSetup {
            problem: "Claktory's browser needs Google Chrome on this Mac for now. Install it from google.com/chrome, then try again."
                .to_owned(),
            ..Default::default()
        })
    } else if cfg!(target_os = "macos") {
        super::mac::problem(home)
    } else {
        Some(unsupported("Claktory's browser runs on Linux and on a Mac, not on this system."))
    }
}

fn unsupported(problem: &str) -> v1::AgentBrowserSetup {
    v1::AgentBrowserSetup { problem: problem.to_owned(), unsupported: true, ..Default::default() }
}

/// Linux: each thing setup installs, in the order a fix would go.
fn linux(home: &Path) -> Option<v1::AgentBrowserSetup> {
    if !matches!(std::env::consts::ARCH, "x86_64" | "aarch64") {
        return Some(unsupported("Claktory's browser runs on 64-bit Intel or ARM computers only."));
    }
    let abi = landlock_abi();
    if abi < 4 {
        return Some(unsupported(&format!(
            "This computer's Linux kernel can't contain the browser (it needs Landlock 4, from Linux 6.7; it has {}). Ubuntu 24.04 or Debian 13 can.",
            if abi == 0 { "none".to_owned() } else { abi.to_string() }
        )));
    }
    let missing = missing();
    if missing.is_empty() {
        return None;
    }
    let person = person();
    let nixos = Path::new("/etc/NIXOS").exists();
    let what = factory_agent::text::list_and(&missing);
    Some(if nixos {
        v1::AgentBrowserSetup {
            problem: format!(
                "Claktory's browser needs a few things set up on this computer first ({what}). Add these lines to your NixOS configuration, save claktory-browser.nft beside it, then run nixos-rebuild switch."
            ),
            nixos_lines: NIXOS.replace("NAME", &person),
            ..Default::default()
        }
    } else {
        v1::AgentBrowserSetup {
            problem: format!(
                "Claktory's browser needs a few things set up on this computer first ({what}). Run this command once in a terminal on this computer; it asks for your password."
            ),
            command: format!("sudo bash {} --user {person}", script(home).display()),
            ..Default::default()
        }
    })
}

/// What's missing of what setup installs, in a few words each.
fn missing() -> Vec<String> {
    let mut missing = Vec::new();
    let chrome = ["/opt/google/chrome/chrome", "/run/current-system/sw/bin/google-chrome-stable", "/run/current-system/sw/bin/chromium"];
    if !chrome.iter().any(|path| Path::new(path).is_file()) {
        missing.push("Google Chrome".to_owned());
    }
    if which("Xvfb").is_none() {
        missing.push("Xvfb".to_owned());
    }
    // Each runner mounts its own over it, and refuses to start unless it's
    // root's (Ghiaccio 38549, Fugo 38527).
    if !x11_folder_ok(Path::new("/tmp/.X11-unix")) {
        missing.push("the display folder".to_owned());
    }
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    if !passwd.lines().any(|line| line.split(':').next() == Some(USER)) {
        missing.push("the browser's own user".to_owned());
    }
    if !Path::new(HOME).is_dir() {
        missing.push("its folder".to_owned());
    }
    if !Path::new(RUNNER_DIR).is_dir() {
        missing.push("the runner's folder".to_owned());
    }
    if !sudo_line() {
        missing.push("its sudo line".to_owned());
    }
    match policy_folder() {
        PolicyFolder::Ready => {}
        PolicyFolder::Missing => missing.push("Chrome's policy folder".to_owned()),
        PolicyFolder::Old => missing.push("removing an old setting that changed your own Chrome".to_owned()),
    }
    // A site sees the fonts a person's computer has (change 3); fontconfig
    // finds them as Chrome does, on NixOS too (Pericolo 37396).
    if !emoji_font() {
        missing.push("its fonts".to_owned());
    }
    // The person's time zone needs the zone files (NixOS keeps its own).
    if !Path::new("/etc/NIXOS").exists() && !Path::new("/usr/share/zoneinfo/UTC").exists() {
        missing.push("time zone data".to_owned());
    }
    // On NixOS the table is the configuration's own, not a file of ours.
    if !Path::new("/etc/NIXOS").exists() && !(Path::new(FIREWALL).is_file() && Path::new(FIREWALL_UNIT).is_file()) {
        missing.push("its firewall rules".to_owned());
    }
    missing
}

/// Whether sudo lets the person run exactly the runner as the browser's
/// user without a password.
fn sudo_line() -> bool {
    let Ok(listed) = std::process::Command::new("sudo").args(["-n", "-l"]).stdin(std::process::Stdio::null()).output() else {
        return false;
    };
    String::from_utf8_lossy(&listed.stdout).lines().any(|line| line.trim() == SUDO_LINE)
}

/// Whether `folder` (/tmp/.X11-unix) is a real folder of root's, as the
/// runner needs it: never a link.
fn x11_folder_ok(folder: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(folder).is_ok_and(|meta| meta.file_type().is_dir() && meta.uid() == 0)
}

/// Whether fontconfig has Noto Color Emoji, the last of setup's fonts.
fn emoji_font() -> bool {
    std::process::Command::new("fc-list")
        .args(["Noto Color Emoji", "family"])
        .stdin(std::process::Stdio::null())
        .output()
        .is_ok_and(|listed| String::from_utf8_lossy(&listed.stdout).lines().any(|line| !line.trim().is_empty()))
}

#[derive(Debug, PartialEq)]
enum PolicyFolder {
    Ready,
    /// Chrome's isn't there as a real folder of root's (a link, or anyone
    /// else's, is refused, as the runner refuses it).
    Missing,
    /// An older setup's policy file is still in one, where the person's own
    /// Chrome reads it.
    Old,
}

fn policy_folder() -> PolicyFolder {
    policy_folder_in(&POLICY_FOLDERS.map(Path::new))
}

/// `folders[0]` is the one the runner needs (Chrome's), as a real folder of
/// root's; none may hold our old policy file. Whatever else is in them (an
/// employer's policy for the person's Chrome) is theirs, never a problem.
fn policy_folder_in(folders: &[&Path]) -> PolicyFolder {
    use std::os::unix::fs::MetadataExt;
    if folders.iter().any(|folder| folder.join(OLD_POLICY).exists()) {
        return PolicyFolder::Old;
    }
    match folders.first().map(std::fs::symlink_metadata) {
        Some(Ok(meta)) if meta.file_type().is_dir() && meta.uid() == 0 => PolicyFolder::Ready,
        _ => PolicyFolder::Missing,
    }
}

/// The kernel's Landlock ABI, 0 with none.
#[cfg(target_os = "linux")]
pub(crate) fn landlock_abi() -> u32 {
    // landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)
    let abi = unsafe { libc::syscall(libc::SYS_landlock_create_ruleset, std::ptr::null::<libc::c_void>(), 0usize, 1u32) };
    u32::try_from(abi).unwrap_or(0)
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn landlock_abi() -> u32 {
    0
}

fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(program)).find(|path| path.is_file())
}

/// Who runs Claktory, by login name.
fn person() -> String {
    std::env::var("USER")
        .ok()
        .filter(|name| !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
        .unwrap_or_else(|| "NAME".to_owned())
}

/// Where this release keeps the setup script: beside the backend, or in its
/// release's scripts folder.
fn script(home: &Path) -> PathBuf {
    let name = "browser-setup-linux.sh";
    let exe = std::env::current_exe().ok();
    let beside = exe.as_ref().and_then(|exe| exe.parent()).map(Path::to_path_buf);
    let candidates = [
        beside.as_ref().map(|dir| dir.join(name)),
        beside.as_ref().map(|dir| dir.join("scripts").join(name)),
        beside.as_ref().and_then(|dir| dir.parent()).map(|dir| dir.join("scripts").join(name)),
        Some(crate::home::Home::new(home.to_path_buf()).releases().join(crate::update::VERSION).join("scripts").join(name)),
    ];
    candidates.into_iter().flatten().find(|path| path.is_file()).unwrap_or_else(|| PathBuf::from(name))
}

/// The NixOS lines (scripts/browser-setup-linux.md), NAME the person.
const NIXOS: &str = r#"{ pkgs, lib, ... }:
let
  person = "NAME";  # who runs Claktory
  # Google's Chrome where nixpkgs has it for this machine, Chromium elsewhere.
  chrome = if lib.meta.availableOn pkgs.stdenv.hostPlatform pkgs.google-chrome then pkgs.google-chrome else pkgs.chromium;
in {
  nixpkgs.config.allowUnfree = true;
  environment.systemPackages = with pkgs; [ chrome xvfb xorg.xauth ];
  # Fonts go in fonts.packages, where fontconfig (and so Chrome) finds them.
  fonts.packages = with pkgs; [ liberation_ttf dejavu_fonts noto-fonts noto-fonts-color-emoji noto-fonts-cjk-sans ];
  users.groups.claktory-browser = {};
  users.users.claktory-browser = { isSystemUser = true; group = "claktory-browser"; home = "/var/lib/claktory-browser"; createHome = true; homeMode = "700"; };
  systemd.tmpfiles.rules = [
    "d /var/lib/claktory-browser/profiles 0700 claktory-browser claktory-browser -"
    "d /opt/claktory-browser 0755 ${person} users -"
    "D! /tmp/.X11-unix 1777 root root -"
  ];
  security.sudo.extraRules = [{
    users = [ person ]; runAs = "claktory-browser";
    commands = [{ command = "/opt/claktory-browser/runner browser-runner *"; options = [ "NOPASSWD" ]; }];
  }];
  # Empty places the runner mounts Chrome's policy over, in its own
  # namespace only: your own Chrome never reads it.
  environment.etc."opt/chrome/policies/managed/claktory-browser-mount.json".text = "{}";
  environment.etc."chromium/policies/managed/claktory-browser-mount.json".text = "{}";
  networking.nftables.enable = true;
  networking.nftables.tables.claktory_browser = { family = "inet"; content = builtins.readFile ./claktory-browser.nft; };
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_display_folder_must_be_roots_and_no_link() {
        assert!(x11_folder_ok(Path::new("/")));
        let dir = std::env::temp_dir().join(format!("x11-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // Not root's.
        assert!(!x11_folder_ok(&dir));
        // A link to a folder of root's isn't one.
        let link = dir.join("link");
        std::os::unix::fs::symlink("/", &link).unwrap();
        assert!(!x11_folder_ok(&link));
        assert!(!x11_folder_ok(&dir.join("missing")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Fugo 49031: the policy lives only in the runner's namespace. Setup's
    /// folder is ready when Chrome's is a real folder of root's with no
    /// policy file of ours left in either; another file there (an
    /// employer's) is fine.
    #[test]
    fn the_policy_folder_is_roots_and_holds_no_policy_of_ours() {
        let dir = std::env::temp_dir().join(format!("policy-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("chromium")).unwrap();
        let chromium = dir.join("chromium");
        // A system folder of root's (never a link: /etc is one on a Mac), as Chrome's, with other files in it.
        assert_eq!(policy_folder_in(&[Path::new("/usr"), &chromium]), PolicyFolder::Ready);
        // Not root's, or a link to root's, or not there.
        assert_eq!(policy_folder_in(&[&dir, &chromium]), PolicyFolder::Missing);
        let link = dir.join("link");
        std::os::unix::fs::symlink("/usr", &link).unwrap();
        assert_eq!(policy_folder_in(&[&link, &chromium]), PolicyFolder::Missing);
        assert_eq!(policy_folder_in(&[&dir.join("missing"), &chromium]), PolicyFolder::Missing);
        // An older setup's policy, in either, is to be taken out.
        std::fs::write(chromium.join("claktory.json"), "{}").unwrap();
        assert_eq!(policy_folder_in(&[Path::new("/usr"), &chromium]), PolicyFolder::Old);
        std::fs::remove_dir_all(&dir).unwrap();
        // The NixOS lines write no policy, only the mount points.
        assert!(!NIXOS.contains("BrowserSignin") && !NIXOS.contains("managed/claktory.json"));
        assert!(NIXOS.contains(r#"environment.etc."opt/chrome/policies/managed/claktory-browser-mount.json".text = "{}";"#));
    }
}
