//! What Claktory's browser needs on this computer (#435 P4, C4): checked
//! before it starts, and shown on the setup card with what fixes it. On
//! Linux: a supported system, Landlock ABI 4 or later, Chrome and Xvfb,
//! the browser's own user and folders, the browser's gate (root's) with
//! the vault's key beside it and the sudo line for exactly the gate, the
//! empty folder the runner mounts Chrome's policy over (no
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
/// The browser's gate (`browser_runner::gate`): a copy of factoryd that
/// setup put there, root's, as is every folder above it.
pub(crate) const GATE: &str = "/usr/local/libexec/claktory-browser/gate";
/// Where Google Chrome and Chromium read managed policy: the runner mounts
/// a tmpfs of its own over them and writes Claktory's policy there, so only
/// its Chrome reads it (Fugo 49031). Setup only makes them, empty and
/// root's, and takes out the policy file older setups wrote in them, which
/// the person's own Chrome read too.
const POLICY_FOLDERS: [&str; 2] = ["/etc/opt/chrome/policies/managed", "/etc/chromium/policies/managed"];
const OLD_POLICY: &str = "claktory.json";
const FIREWALL: &str = "/etc/claktory-browser.nft";
/// An older setup's AppArmor profile letting the runner's path, which the
/// person can write, make user namespaces: setup takes it out.
const OLD_RUNNER_PROFILE: &str = "/etc/apparmor.d/claktory-browser-runner";
const FIREWALL_UNIT: &str = "/etc/systemd/system/claktory-browser-nft.service";
/// What `sudo -n -l` lists for the gate when the sudo line is there.
const SUDO_LINE: &str = "(claktory-browser) NOPASSWD: /usr/local/libexec/claktory-browser/gate browser-gate run";

/// The Mac's own Chrome, which the dev stand-in drives.
pub(crate) const MAC_CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

static CHECKED: Mutex<Option<(Instant, Option<v1::AgentBrowserSetup>)>> = Mutex::new(None);
/// The public half of the gate's key, as setup writes it: read from the
/// vault at each `problem` (never under the Factory's lock), for the checks
/// in between. None until the vault has been open.
static RUNNER_KEY: Mutex<Option<String>> = Mutex::new(None);
static CHECKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The answer changed since the Workspace last read it (`watch`).
static CHANGED: tokio::sync::Notify = tokio::sync::Notify::const_new();

/// Keeps `answer` as the last one; when it differs from the one before,
/// every client is told (`watch`), rather than at the next Workspace sent
/// for something else (Abbacchio 1782: a failed image load said "getting
/// ready…" for half a minute).
fn keep(answer: Option<v1::AgentBrowserSetup>) {
    let mut checked = CHECKED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let differs = checked.as_ref().is_none_or(|(_, before)| *before != answer);
    *checked = Some((Instant::now(), answer));
    drop(checked);
    if differs {
        CHANGED.notify_one();
    }
}

/// Checks again now, from a thread of its own (it runs `container`, or
/// `sudo -n -l`): after the browser's image was got, or couldn't be.
pub(crate) fn recheck(home: &Path) {
    keep(check(home, known_key().as_deref()));
}

fn known_key() -> Option<String> {
    RUNNER_KEY.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

/// The vault's key for the gate, in `KEY_FILE`'s words, kept for the
/// checks after; the last one known while the vault is locked.
fn runner_key(factory: &Factory) -> Option<String> {
    if cfg!(target_os = "linux")
        && let Ok(key) = factory.vault.browser_runner_key()
    {
        *RUNNER_KEY.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(crate::browser_runner::gate::key_text(&key.verifying_key()));
    }
    known_key()
}

/// How often, on Linux, the files setup writes are looked at for a change:
/// the person ran its command in a terminal, or something undid it.
const LOOKED_AT: Duration = Duration::from_secs(5);

/// Sends the Workspace again each time the setup's answer changes. On
/// Linux it first reads the gate's key from the vault, so the first setup
/// card already carries it; then it checks again as soon as a file setup
/// writes changes, rather than at the next start or Workspace (only
/// looking at them in between: a whole check runs `sudo -l`, which the
/// system logs).
pub(crate) async fn watch(factory: std::sync::Weak<Factory>) {
    let linux = cfg!(target_os = "linux");
    if linux && let Some(factory) = factory.upgrade() {
        let _ = tokio::task::spawn_blocking(move || runner_key(&factory)).await;
    }
    let mut seen = if linux { tokio::task::spawn_blocking(setup_files).await.ok() } else { None };
    let mut look = tokio::time::interval(LOOKED_AT);
    look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    look.tick().await;
    loop {
        if !linux {
            CHANGED.notified().await;
        } else {
            tokio::select! {
                () = CHANGED.notified() => {}
                _ = look.tick() => {
                    let now = tokio::task::spawn_blocking(setup_files).await.unwrap_or_default();
                    if seen.replace(now.clone()).is_some_and(|before| before != now)
                        && let Some(factory) = factory.upgrade()
                    {
                        let home = factory.home().root().to_path_buf();
                        drop(factory);
                        let _ = tokio::task::spawn_blocking(move || recheck(&home)).await;
                    }
                    continue;
                }
            }
        }
        let Some(factory) = factory.upgrade() else { return };
        factory.broadcast_workspace(&factory.lock());
    }
}

/// A file as `setup_files` looks at it: inode, mode, owner, its times.
type Looked = Option<(u64, u32, u32, i64, i64, i64)>;

/// What the files setup writes look like now (which there are, their
/// owners, modes and times), apt's record of what's installed with them.
fn setup_files() -> Vec<Looked> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let seed = Path::new(crate::browser_runner::safe_browsing::SEED);
        let mut paths: Vec<PathBuf> = [
            GATE,
            crate::browser_runner::gate::KEY_FILE,
            "/etc/sudoers.d/claktory-browser",
            OLD_RUNNER_PROFILE,
            FIREWALL,
            FIREWALL_UNIT,
            HOME,
            RUNNER_DIR,
            "/etc/passwd",
            "/tmp/.X11-unix",
            "/var/lib/dpkg/status",
            crate::browser_runner::safe_browsing::SEED,
        ]
        .iter()
        .chain(POLICY_FOLDERS.iter())
        .map(PathBuf::from)
        .collect();
        if let Ok(entries) = std::fs::read_dir(seed) {
            let mut each: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
            each.sort();
            paths.extend(each);
        }
        paths
            .iter()
            .map(|path| std::fs::symlink_metadata(path).ok().map(|meta| (meta.ino(), meta.mode(), meta.uid(), meta.mtime(), meta.ctime(), meta.ctime_nsec())))
            .collect()
    }
    #[cfg(windows)]
    {
        Vec::new()
    }
}

/// Why it can't run here, with what fixes it; None when it can. Read now
/// (it may run `sudo -n -l`): never under the Factory's lock.
pub(crate) fn problem(factory: &Factory) -> Option<v1::AgentBrowserSetup> {
    let answer = check(factory.home().root(), runner_key(factory).as_deref());
    keep(answer.clone());
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
            keep(check(&home, known_key().as_deref()));
            CHECKING.store(false, std::sync::atomic::Ordering::Release);
        });
    }
    checked.and_then(|(_, answer)| answer)
}

fn check(home: &Path, runner_key: Option<&str>) -> Option<v1::AgentBrowserSetup> {
    if cfg!(target_os = "linux") {
        linux(home, runner_key)
    } else if cfg!(target_os = "macos") && super::standin_here(home) {
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
fn linux(home: &Path, runner_key: Option<&str>) -> Option<v1::AgentBrowserSetup> {
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
    // The gate runs the runner from a sealed copy in memory.
    if std::fs::read_to_string("/proc/sys/vm/memfd_noexec").is_ok_and(|value| value.trim() == "2") {
        return Some(unsupported(
            "This computer doesn't let programs run from memory (vm.memfd_noexec is 2), which Claktory's browser needs. Setting it to 1 lets it run.",
        ));
    }
    let gate = gate_in_place(Path::new(GATE));
    let missing = missing(gate, runner_key);
    if missing.is_empty() {
        return None;
    }
    let person = person();
    let nixos = Path::new("/etc/NIXOS").exists();
    let what = factory_agent::text::list_and(&missing);
    let own = gate_source();
    Some(if nixos {
        v1::AgentBrowserSetup {
            problem: if gate {
                format!(
                    "Claktory's browser needs a few things set up on this computer first ({what}). Add these lines to your NixOS configuration, save claktory-browser.nft beside it, then run nixos-rebuild switch."
                )
            } else {
                format!(
                    "Claktory's browser needs a few things set up on this computer first ({what}). Run this command once in a terminal on this computer (it asks for your password), add these lines to your NixOS configuration, save claktory-browser.nft beside it, then run nixos-rebuild switch."
                )
            },
            command: if gate { String::new() } else { nixos_gate_command(&own) },
            nixos_lines: nixos_lines(&person, runner_key),
            ..Default::default()
        }
    } else {
        let key = runner_key.map_or(String::new(), |key| format!(" --runner-key {key}"));
        v1::AgentBrowserSetup {
            problem: format!(
                "Claktory's browser needs a few things set up on this computer first ({what}). Run this command once in a terminal on this computer; it asks for your password."
            ),
            command: format!("sudo bash {} --user {person} --gate {}{key}", shell_word(&script(home).display().to_string()), shell_word(&own)),
            ..Default::default()
        }
    })
}

/// Whether the gate is in place: a file of root's, every folder above it
/// too, none a link, that speaks this factoryd's gate protocol.
fn gate_in_place(gate: &Path) -> bool {
    #[cfg(unix)]
    {
        if crate::browser_runner::gate::roots_alone(gate).is_err() || !std::fs::symlink_metadata(gate).is_ok_and(|meta| meta.is_file()) {
            return false;
        }
        let said = std::process::Command::new(gate)
            .args(["browser-gate", "version"])
            .env_clear()
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        said.is_ok_and(|said| said.status.success() && String::from_utf8_lossy(&said.stdout).trim() == crate::browser_runner::gate::VERSION.to_string())
    }
    // No root on Windows, where the runner never runs (`check`).
    #[cfg(windows)]
    {
        let _ = gate;
        false
    }
}

/// What setup copies to the gate's place: this factoryd's program.
fn gate_source() -> String {
    std::env::current_exe().map(|exe| exe.display().to_string()).unwrap_or_else(|_| "factoryd".to_owned())
}

/// NixOS's one step outside its configuration: the gate's copy, root's.
fn nixos_gate_command(own: &str) -> String {
    format!("sudo install -D -o root -g root -m 0755 {} {GATE}", shell_word(own))
}

/// `word` as one word for the shell.
fn shell_word(word: &str) -> String {
    if !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"/._-+:=@%,".contains(&byte)) {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

/// What's missing of what setup installs, in a few words each.
fn missing(gate: bool, runner_key: Option<&str>) -> Vec<String> {
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
    if !gate {
        missing.push("the safeguard that lets only Claktory start it".to_owned());
    }
    if runner_key.is_some_and(|key| !runner_key_in_place(Path::new(crate::browser_runner::gate::KEY_FILE), key)) {
        missing.push("the browser's key".to_owned());
    }
    if !sudo_line() {
        missing.push("its sudo line".to_owned());
    }
    // A new profile's Safe Browsing lists until Chrome fetches its own; the
    // NixOS lines don't bring them, and sign-ins and payments wait there.
    if !Path::new("/etc/NIXOS").exists() && !safe_browsing_seed() {
        missing.push("Safe Browsing's lists".to_owned());
    }
    if std::fs::symlink_metadata(OLD_RUNNER_PROFILE).is_ok() {
        missing.push("removing an old permission the browser no longer needs".to_owned());
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

/// Whether setup's Safe Browsing seed is in place.
#[cfg(unix)]
fn safe_browsing_seed() -> bool {
    crate::browser_runner::safe_browsing::seed_in_place(Path::new(crate::browser_runner::safe_browsing::SEED))
}

// No root on Windows, where the runner never runs (`check`).
#[cfg(windows)]
fn safe_browsing_seed() -> bool {
    false
}

/// Whether `file` holds `key` and is root's alone, as the gate reads it.
fn runner_key_in_place(file: &Path, key: &str) -> bool {
    #[cfg(unix)]
    {
        crate::browser_runner::gate::roots_alone(file).is_ok()
            && std::fs::symlink_metadata(file).is_ok_and(|meta| meta.is_file())
            && std::fs::read_to_string(file).is_ok_and(|text| text.trim() == key)
    }
    // No root on Windows, where the runner never runs (`check`).
    #[cfg(windows)]
    {
        let _ = (file, key);
        false
    }
}

/// Whether sudo lets the person run exactly the gate as the browser's user
/// without a password, and nothing else of the browser's.
fn sudo_line() -> bool {
    let Ok(listed) = std::process::Command::new("sudo").args(["-n", "-l"]).stdin(std::process::Stdio::null()).output() else {
        return false;
    };
    sudo_lines_ok(&String::from_utf8_lossy(&listed.stdout))
}

/// `sudo -n -l`'s list: the gate's line, and no line naming the runner,
/// which an older setup let the person run as the browser's user directly.
fn sudo_lines_ok(listed: &str) -> bool {
    listed.lines().any(|line| line.trim() == SUDO_LINE) && !listed.lines().any(|line| line.contains(RUNNER))
}

/// Whether `folder` (/tmp/.X11-unix) is a real folder of root's, as the
/// runner needs it: never a link.
fn x11_folder_ok(folder: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(folder).is_ok_and(|meta| meta.file_type().is_dir() && meta.uid() == 0)
    }
    // No root on Windows, where the runner never runs (`check`).
    #[cfg(windows)]
    {
        let _ = folder;
        false
    }
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
    #[cfg_attr(windows, allow(dead_code, reason = "Chrome's policy folder is never ready on Windows: setup refuses there"))]
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
    if folders.iter().any(|folder| folder.join(OLD_POLICY).exists()) {
        return PolicyFolder::Old;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match folders.first().map(std::fs::symlink_metadata) {
            Some(Ok(meta)) if meta.file_type().is_dir() && meta.uid() == 0 => PolicyFolder::Ready,
            _ => PolicyFolder::Missing,
        }
    }
    // No root on Windows, where the runner never runs (`check`).
    #[cfg(windows)]
    PolicyFolder::Missing
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

/// Who runs Claktory, by login name: USER, or else the system's name for
/// this process's user (a service or `podman exec` may set no USER).
fn person() -> String {
    let fits = |name: &String| !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    std::env::var("USER").ok().filter(fits).or_else(|| own_login().filter(fits)).unwrap_or_else(|| "NAME".to_owned())
}

#[cfg(unix)]
fn own_login() -> Option<String> {
    // SAFETY: passwd is plain C data, all zeroes a valid value.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: getpwuid_r with our own entry and buffer, which outlive it;
    // the name it points into is copied before they go.
    let failed = unsafe { libc::getpwuid_r(libc::getuid(), &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found) };
    if failed != 0 || found.is_null() || entry.pw_name.is_null() {
        return None;
    }
    // SAFETY: a NUL-ended name inside `buffer`, still alive.
    Some(unsafe { std::ffi::CStr::from_ptr(entry.pw_name) }.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn own_login() -> Option<String> {
    None
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

/// The NixOS lines for `person`, with the gate's key when it's known.
fn nixos_lines(person: &str, runner_key: Option<&str>) -> String {
    let key = runner_key.map_or(String::new(), |key| {
        format!("  # The browser's key: the gate runs the runner only for Claktory, which holds its other half.\n  environment.etc.\"claktory-browser/runner.pub\" = {{ text = \"{key}\\n\"; mode = \"0444\"; }};\n")
    });
    NIXOS.replace("NAME", person).replace("  networking.nftables.enable = true;\n", &format!("{key}  networking.nftables.enable = true;\n"))
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
  # The browser's gate, the one program you may run as its user; it runs
  # the runner only for Claktory. Put it in place with the command beside
  # these lines.
  security.sudo.extraRules = [{
    users = [ person ]; runAs = "claktory-browser";
    commands = [{ command = "/usr/local/libexec/claktory-browser/gate browser-gate run"; options = [ "NOPASSWD" ]; }];
  }];
  # Empty places the runner mounts Chrome's policy over, in its own
  # namespace only: your own Chrome never reads it.
  environment.etc."opt/chrome/policies/managed/claktory-browser-mount.json".text = "{}";
  environment.etc."chromium/policies/managed/claktory-browser-mount.json".text = "{}";
  networking.nftables.enable = true;
  networking.nftables.tables.claktory_browser = { family = "inet"; content = builtins.readFile ./claktory-browser.nft; };
}"#;

// Claktory's browser isn't on Windows (`check` says so).
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// The setup card names the person even when nothing set USER.
    #[test]
    fn the_person_is_named_by_the_system_without_user() {
        let name = own_login().expect("this user has a name");
        assert!(!name.is_empty() && name != "NAME");
        // SAFETY: getuid can't fail; getpwnam with a C string of ours.
        let uid = unsafe { libc::getpwnam(std::ffi::CString::new(name.clone()).unwrap().as_ptr()).as_ref().map(|entry| entry.pw_uid) };
        assert_eq!(uid, Some(unsafe { libc::getuid() }), "{name} is this process's user");
    }

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

    /// The gate's line is the person's only way to the browser's user: an
    /// older setup's line for the runner itself, still there beside it,
    /// means setup hasn't run again yet.
    #[test]
    fn sudo_lets_the_person_run_the_gate_and_nothing_else_of_the_browsers() {
        let gate = "User matt may run the following commands on prime:\n    (ALL : ALL) ALL\n    (claktory-browser) NOPASSWD: /usr/local/libexec/claktory-browser/gate browser-gate run\n";
        assert!(sudo_lines_ok(gate));
        let old = "    (claktory-browser) NOPASSWD: /opt/claktory-browser/runner browser-runner *\n";
        assert!(!sudo_lines_ok(old));
        assert!(!sudo_lines_ok(&format!("{gate}{old}")), "the runner's line is still there");
        assert!(!sudo_lines_ok("    (claktory-browser) NOPASSWD: /usr/local/libexec/claktory-browser/gate browser-gate *\n"), "any arguments");
        assert!(!sudo_lines_ok(""));
    }

    /// The NixOS lines carry the gate's key as a file of its own (a mode
    /// makes it a copy, not a link into the store) and the gate's sudo
    /// line; the gate and the key are refused anywhere the person could
    /// write.
    #[test]
    fn setup_puts_the_gate_and_its_key_where_only_root_writes() {
        let key = "ab".repeat(32);
        let lines = nixos_lines("matt", Some(&key));
        assert!(lines.contains(&format!(r#"environment.etc."claktory-browser/runner.pub" = {{ text = "{key}\n"; mode = "0444"; }};"#)), "{lines}");
        assert!(lines.contains(r#"command = "/usr/local/libexec/claktory-browser/gate browser-gate run";"#), "{lines}");
        assert!(!lines.contains("browser-runner *"), "{lines}");
        assert!(lines.contains("person = \"matt\""));
        assert!(!nixos_lines("matt", None).contains("runner.pub"));
        assert_eq!(nixos_gate_command("/home/matt/.factory/bin/factoryd"), "sudo install -D -o root -g root -m 0755 /home/matt/.factory/bin/factoryd /usr/local/libexec/claktory-browser/gate");
        assert_eq!(shell_word("/home/matt/my factory/it's"), r"'/home/matt/my factory/it'\''s'");

        let dir = std::env::temp_dir().join(format!("gate-setup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("runner.pub");
        std::fs::write(&file, format!("{key}\n")).unwrap();
        assert!(!runner_key_in_place(&file, &key), "the person's own file");
        assert!(!runner_key_in_place(&dir.join("none"), &key));
        let gate = dir.join("gate");
        std::fs::copy(std::env::current_exe().unwrap(), &gate).unwrap();
        assert!(!gate_in_place(&gate), "the person's own copy");
        assert!(!gate_in_place(&dir.join("none")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
