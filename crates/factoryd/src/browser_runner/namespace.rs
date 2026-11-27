//! The runner's own /proc (Fugo 36536, 36609). /proc/<pid>/environ of the
//! person's processes is closed to the browser's user, but their cmdline
//! isn't: an agent's `curl -H "Authorization: Bearer …"` holds the value
//! in its arguments. So before anything else, while it's still a single
//! thread, the runner makes its own user, PID and mount namespaces (its uid
//! mapped to itself, never root), mounts a /proc of its own there (with
//! hidepid where the kernel takes it), checks that it sees nothing outside,
//! and drops every capability the namespace gave it, so neither it nor
//! anything it starts can unmount that /proc again. It's PID 1 there: when
//! it ends, the kernel ends Chrome and the screen with it.
//!
//! On a Linux server the gate does this (`enter_for_gate`) and runs the
//! runner inside, which checks it finds itself so (`gated`); inside the
//! Mac's VM the runner does it itself (`enter`).
//!
//! It fails closed: a runner that can't do this doesn't run. On Ubuntu
//! 23.10 and later that needs the AppArmor profile setup installs for the
//! gate's path; Debian and NixOS allow it as they are.

use std::ffi::CString;
use std::sync::atomic::{AtomicI32, Ordering};

/// The child, for the parent's signal handler to pass signals on to.
static CHILD: AtomicI32 = AtomicI32::new(0);

extern "C" fn pass_on(signal: libc::c_int) {
    let child = CHILD.load(Ordering::Relaxed);
    if child > 0 {
        // SAFETY: kill is async-signal-safe.
        unsafe { libc::kill(child, signal) };
    }
}

/// Enters the runner's namespaces. Returns in the child, PID 1 of the new
/// PID namespace with its own /proc and no capabilities; the parent stays
/// outside, passes signals on, and exits as the child does. Err: why it
/// can't (the parent then reports it and exits).
///
/// Must run before the process has a second thread.
pub(crate) fn enter() -> Result<(), String> {
    enter_with(Policy::Written)
}

/// The gate's: the same, with Chrome's policy folders mounted empty and
/// left for the runner it runs to write (`gated`), so the policy is always
/// the runner's own, whichever factoryd made the gate.
pub(crate) fn enter_for_gate() -> Result<(), String> {
    enter_with(Policy::LeftToTheRunner)
}

/// Who writes Chrome's policy into the folders mounted for it.
#[derive(Clone, Copy, PartialEq)]
enum Policy {
    Written,
    LeftToTheRunner,
}

fn enter_with(policy: Policy) -> Result<(), String> {
    // Before the user namespace, where root shows as nobody.
    x_sockets_folder_ok()?;
    let policy_folders = policy_folders()?;
    let seed = if super::safe_browsing::usable(std::path::Path::new(super::safe_browsing::SEED)) { Seed::Kept } else { Seed::Hidden };
    // SAFETY: getuid and getgid can't fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    // SAFETY: a single-threaded process unsharing its own namespaces.
    if unsafe { libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWPID | libc::CLONE_NEWNS) } != 0 {
        return Err(format!(
            "Linux won't let Claktory's browser have a /proc of its own ({}): run Claktory's browser setup again",
            std::io::Error::last_os_error()
        ));
    }
    write("/proc/self/setgroups", "deny")?;
    write("/proc/self/uid_map", &format!("{uid} {uid} 1"))?;
    write("/proc/self/gid_map", &format!("{gid} {gid} 1"))?;
    // SAFETY: still a single thread.
    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(format!("couldn't start the browser's runner: {}", std::io::Error::last_os_error()));
    }
    if child > 0 {
        parent(child);
    }
    inside(&policy_folders, policy, seed)
}

/// What becomes of Safe Browsing's seed in the namespace, as decided
/// before it.
#[derive(Clone, Copy, PartialEq)]
enum Seed {
    Kept,
    Hidden,
}

/// The parent, outside: passes its signals on and ends as the child does.
fn parent(child: libc::pid_t) -> ! {
    CHILD.store(child, Ordering::Relaxed);
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: a handler that only calls kill.
        unsafe { libc::signal(signal, pass_on as *const () as libc::sighandler_t) };
    }
    let mut status = 0;
    loop {
        // SAFETY: our own child.
        let waited = unsafe { libc::waitpid(child, &mut status, 0) };
        if waited == child {
            break;
        }
        if waited < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            std::process::exit(1);
        }
    }
    let code = if libc::WIFEXITED(status) { libc::WEXITSTATUS(status) } else { 128 + libc::WTERMSIG(status) };
    std::process::exit(code);
}

/// PID 1, inside.
fn inside(policy_folders: &[&str], policy: Policy, seed: Seed) -> Result<(), String> {
    // Gone with the parent (factoryd's sudo ended, say): the kernel then
    // ends everything in the namespace.
    // SAFETY: a prctl on ourselves.
    unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) };
    let root = CString::new("/").unwrap_or_default();
    let proc = CString::new("/proc").unwrap_or_default();
    let kind = CString::new("proc").unwrap_or_default();
    // SAFETY: mounts in our own mount namespace, with C strings of ours.
    unsafe {
        // Nothing of ours propagates back out.
        if libc::mount(std::ptr::null(), root.as_ptr(), std::ptr::null(), libc::MS_REC | libc::MS_PRIVATE, std::ptr::null()) != 0 {
            return Err(format!("couldn't make the browser's mounts its own: {}", std::io::Error::last_os_error()));
        }
        let flags = libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
        let hidden = CString::new("hidepid=invisible").unwrap_or_default();
        if libc::mount(kind.as_ptr(), proc.as_ptr(), kind.as_ptr(), flags, hidden.as_ptr().cast()) != 0
            && libc::mount(kind.as_ptr(), proc.as_ptr(), kind.as_ptr(), flags, std::ptr::null()) != 0
        {
            return Err(format!("couldn't give the browser a /proc of its own: {}", std::io::Error::last_os_error()));
        }
    }
    own_x_sockets()?;
    own_policy(policy_folders, policy)?;
    if seed == Seed::Hidden {
        hide_seed()?;
    }
    sees_only_itself()?;
    drop_capabilities()?;
    // SAFETY: a prctl on ourselves.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(format!("couldn't lock the browser's privileges: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// The X socket folder the runner mounts over (Fugo 38527): it must be
/// root's, made by the system or by setup (as root, 1777), never by the
/// runner itself, so the host's folder never belongs to the browser's
/// user; a planted one (a link, or anyone else's) isn't trusted.
fn x_sockets_folder_ok() -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata("/tmp/.X11-unix") {
        Ok(found) if found.is_dir() && found.uid() == 0 => Ok(()),
        Ok(_) => Err("/tmp/.X11-unix isn't a folder of root's, so Claktory's browser won't use it: run Claktory's browser setup again".to_owned()),
        Err(_) => Err("this computer has no /tmp/.X11-unix, which Claktory's browser needs: run Claktory's browser setup again".to_owned()),
    }
}

/// A /tmp/.X11-unix of the runner's own, an empty tmpfs only its mount
/// namespace sees. Xvfb takes the first display number whose lock's
/// process is gone, and another runner's Xvfb is a process it can't see
/// (each has its own PID namespace): so two runners both took :0, and the
/// first to stop removed the other's socket, leaving its Chrome no screen
/// (the end-to-end run on the box, 26 Nov). Its own folder, its own
/// sockets; no other local user reaches them either.
fn own_x_sockets() -> Result<(), String> {
    let at = CString::new("/tmp/.X11-unix").unwrap_or_default();
    let kind = CString::new("tmpfs").unwrap_or_default();
    let options = CString::new("mode=0700,size=64k").unwrap_or_default();
    // SAFETY: a mount in our own mount namespace, with C strings of ours.
    if unsafe { libc::mount(kind.as_ptr(), at.as_ptr(), kind.as_ptr(), libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, options.as_ptr().cast()) } != 0 {
        return Err(format!("couldn't give the browser's screen a folder of its own: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// A seed that isn't root's alone, under an empty read-only folder: the
/// runner, in here, can't tell root's files from anyone else's.
fn hide_seed() -> Result<(), String> {
    let seed = super::safe_browsing::SEED;
    if !std::fs::symlink_metadata(seed).is_ok_and(|meta| meta.is_dir()) {
        return Ok(());
    }
    let at = CString::new(seed).unwrap_or_default();
    let kind = CString::new("tmpfs").unwrap_or_default();
    let options = CString::new("mode=0555,size=4k").unwrap_or_default();
    let flags = libc::MS_RDONLY | libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC;
    // SAFETY: a mount in our own mount namespace, with C strings of ours.
    if unsafe { libc::mount(kind.as_ptr(), at.as_ptr(), kind.as_ptr(), flags, options.as_ptr().cast()) } != 0 {
        return Err(format!("couldn't keep Safe Browsing's seed from the browser: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Chrome's managed policy (C4; Fugo 49031): pages kept off the local
/// network, no sign-in to Chrome itself or sync, so a site's sign-in stays
/// that site's cookies (Fugo 41236), no warning bar for the runner's fixed
/// flags (R11, Fugo 41700), and Chrome never filling or saving a card or an
/// address itself (W9, Fugo 41400), nor saving a password into the profile
/// (logins live only in the vault, guard 10), no site's cookies sent to
/// another's frames (guard 4), and Safe Browsing on with no way past its
/// warning (guard 3: a flagged page is only ever Chrome's error page).
/// LocalNetworkAccessRestrictionsEnabled is for Chromes before 158, which
/// call it unknown.
pub(crate) const POLICY: &str = r#"{"LocalNetworkAccessRestrictionsEnabled": true, "LocalNetworkAccessBlockedForUrls": ["*"], "BrowserSignin": 0, "SyncDisabled": true, "CommandLineFlagSecurityWarningsEnabled": false, "AutofillCreditCardEnabled": false, "AutofillAddressEnabled": false, "PasswordManagerEnabled": false, "BlockThirdPartyCookies": true, "SafeBrowsingProtectionLevel": 1, "DisableSafeBrowsingProceedAnyway": true}"#;

/// Where Google Chrome and Chromium read their managed policy on Linux.
pub(crate) const CHROME_POLICY: &str = "/etc/opt/chrome/policies/managed";
pub(crate) const CHROMIUM_POLICY: &str = "/etc/chromium/policies/managed";
/// The policy's file in them, which only this namespace's tmpfs holds.
pub(crate) const POLICY_FILE: &str = "claktory.json";

/// The policy folders the runner mounts its own over: each that's a real
/// folder of root's (setup makes Chrome's; a person's Chromium may have its
/// own), and Chrome's always, since that's the one Google's Chrome reads,
/// and the Chromium one when Chromium is the browser. Read before the user
/// namespace, where root shows as nobody; a link or anyone else's folder
/// is refused, never used.
fn policy_folders() -> Result<Vec<&'static str>, String> {
    use std::os::unix::fs::MetadataExt;
    let read = read_by_the_browser();
    let mut folders = Vec::new();
    for folder in [CHROME_POLICY, CHROMIUM_POLICY] {
        let needed = folder == read;
        match std::fs::symlink_metadata(folder) {
            Ok(found) if found.is_dir() && found.uid() == 0 => folders.push(folder),
            Ok(_) => return Err(format!("{folder} isn't a folder of root's, so Claktory's browser won't use it: run Claktory's browser setup again")),
            Err(_) if needed => {
                return Err(format!("this computer has no {folder}, where Claktory's browser keeps Chrome's policy: run Claktory's browser setup again"));
            }
            Err(_) => {}
        }
    }
    Ok(folders)
}

/// Chrome's policy, only in this mount namespace (Fugo 49031): a tmpfs of
/// its own over each policy folder, holding `POLICY`, so the person's own
/// Chrome on this computer never reads it (and the browser stays Google's
/// Chrome, with its updates). Fails closed: no policy, no runner.
fn own_policy(folders: &[&str], policy: Policy) -> Result<(), String> {
    let kind = CString::new("tmpfs").unwrap_or_default();
    let options = CString::new("mode=0755,size=64k").unwrap_or_default();
    for folder in folders {
        let at = CString::new(*folder).unwrap_or_default();
        // SAFETY: a mount in our own mount namespace, with C strings of ours.
        if unsafe { libc::mount(kind.as_ptr(), at.as_ptr(), kind.as_ptr(), libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC, options.as_ptr().cast()) } != 0 {
            return Err(format!("couldn't give Chrome's policy a folder of its own ({folder}): {}", std::io::Error::last_os_error()));
        }
        if policy == Policy::Written {
            write_policy(folder)?;
        }
    }
    if policy == Policy::Written {
        policy_in_place()?;
    }
    Ok(())
}

fn write_policy(folder: &str) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let path = std::path::Path::new(folder).join(POLICY_FILE);
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, POLICY.as_bytes()));
    written.map_err(|error| format!("couldn't write Chrome's policy ({}): {error}", path.display()))
}

/// The runner the gate ran (`browser-runner run-gated`), before anything
/// else: it must be what the gate makes it, PID 1 of namespaces of its
/// own, seeing no process outside, with no new privileges, no capability
/// and the browser user's uid. Then it writes its own Chrome policy into
/// the folders the gate mounted for it.
pub(crate) fn gated() -> Result<(), String> {
    // SAFETY: getpid and getuid can't fail; PR_GET_NO_NEW_PRIVS only reads.
    let (pid, uid, no_new_privs) = unsafe { (libc::getpid(), libc::getuid(), libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0)) };
    if pid != 1 {
        return Err("the browser's runner isn't PID 1 of its own namespace: only Claktory's browser gate runs it".to_owned());
    }
    if no_new_privs != 1 {
        return Err("the browser's runner could still gain privileges: only Claktory's browser gate runs it".to_owned());
    }
    if !capabilities_empty()? {
        return Err("the browser's runner still has capabilities: only Claktory's browser gate runs it".to_owned());
    }
    if Some(uid) != browser_uid() {
        return Err(format!("the browser's runner runs as uid {uid}, not as {}", crate::browser_host::setup::USER));
    }
    sees_only_itself()?;
    let mounts = std::fs::read_to_string("/proc/self/mounts").map_err(|error| format!("couldn't read the browser's mounts: {error}"))?;
    for folder in [CHROME_POLICY, CHROMIUM_POLICY] {
        let own = mounts.lines().rfind(|line| line.split_whitespace().nth(1) == Some(folder)).is_some_and(|line| line.starts_with("tmpfs "));
        if own {
            write_policy(folder)?;
        }
    }
    policy_in_place().map(drop)
}

/// The browser user's uid, as the system's users say.
fn browser_uid() -> Option<libc::uid_t> {
    let name = CString::new(crate::browser_host::setup::USER).ok()?;
    // SAFETY: getpwnam with a C string of ours; the entry is read at once,
    // in this single-threaded start.
    let entry = unsafe { libc::getpwnam(name.as_ptr()) };
    // SAFETY: a non-null entry getpwnam returned.
    (!entry.is_null()).then(|| unsafe { (*entry).pw_uid })
}

/// The policy folder the browser reads: Chromium's when the browser is
/// Chromium (NixOS where nixpkgs has no Google Chrome), else Chrome's.
fn read_by_the_browser() -> &'static str {
    let chromium = super::chrome::binary().is_ok_and(|binary| binary.to_string_lossy().contains("chromium"));
    if chromium { CHROMIUM_POLICY } else { CHROME_POLICY }
}

/// Whether Chrome's policy is this namespace's own: a tmpfs at the policy
/// folder the browser reads, holding exactly `POLICY`, and the same in any
/// other it mounted. The selftest's check, and Chrome's before each start:
/// never a Chrome without it (Fugo 49031).
pub(crate) fn policy_in_place() -> Result<String, String> {
    let mounts = std::fs::read_to_string("/proc/self/mounts").map_err(|error| format!("couldn't read the browser's mounts: {error}"))?;
    let read = read_by_the_browser();
    let mut found = Vec::new();
    for folder in [CHROME_POLICY, CHROMIUM_POLICY] {
        let own = mounts.lines().rfind(|line| line.split_whitespace().nth(1) == Some(folder)).is_some_and(|line| line.starts_with("tmpfs "));
        if !own {
            if folder == read {
                return Err(format!("Chrome's policy isn't the browser's own: {folder} isn't a folder of its own"));
            }
            continue;
        }
        let text = std::fs::read_to_string(std::path::Path::new(folder).join(POLICY_FILE)).unwrap_or_default();
        if text != POLICY {
            return Err(format!("Chrome's policy in {folder} isn't Claktory's"));
        }
        found.push(folder);
    }
    Ok(found.join(", "))
}

/// The fail-closed check (Fugo 36609): /proc is this namespace's, so this
/// process is 1 there, and no other process shows.
fn sees_only_itself() -> Result<(), String> {
    let me = std::fs::read_link("/proc/self").map_err(|error| format!("couldn't read the browser's /proc: {error}"))?;
    if me.as_os_str() != "1" {
        return Err(format!("the browser's /proc isn't its own (it shows this process as {})", me.display()));
    }
    let entries = std::fs::read_dir("/proc").map_err(|error| format!("couldn't read the browser's /proc: {error}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.bytes().all(|byte| byte.is_ascii_digit()) && name != "1" {
            return Err(format!("the browser's /proc shows a process outside it ({name})"));
        }
    }
    Ok(())
}

/// Every capability the new user namespace gave this process, dropped:
/// effective, permitted and inheritable, so nothing it starts gets one.
fn drop_capabilities() -> Result<(), String> {
    let header = CapHeader { version: CAP_VERSION_3, pid: 0 };
    let data = [CapData { effective: 0, permitted: 0, inheritable: 0 }; 2];
    // SAFETY: capset with a v3 header and its two data words.
    if unsafe { libc::syscall(libc::SYS_capset, &header as *const CapHeader, data.as_ptr()) } != 0 {
        return Err(format!("couldn't drop the browser's capabilities: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Whether this process has no capability, effective or permitted.
fn capabilities_empty() -> Result<bool, String> {
    let header = CapHeader { version: CAP_VERSION_3, pid: 0 };
    let mut data = [CapData { effective: 0, permitted: 0, inheritable: 0 }; 2];
    // SAFETY: capget with a v3 header and room for its two data words.
    if unsafe { libc::syscall(libc::SYS_capget, &header as *const CapHeader, data.as_mut_ptr()) } != 0 {
        return Err(format!("couldn't read the browser's capabilities: {}", std::io::Error::last_os_error()));
    }
    Ok(data.iter().all(|word| word.effective == 0 && word.permitted == 0))
}

#[repr(C)]
struct CapHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

const CAP_VERSION_3: u32 = 0x2008_0522;

fn write(path: &str, what: &str) -> Result<(), String> {
    std::fs::write(path, what).map_err(|error| format!("couldn't map the browser's user ({path}): {error}"))
}
