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
//! It fails closed: a runner that can't do this doesn't run. On Ubuntu
//! 23.10 and later that needs the AppArmor profile setup installs for the
//! runner's path; Debian and NixOS allow it as they are.

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
    // Before the user namespace, where root shows as nobody.
    x_sockets_folder_ok()?;
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
    inside()
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
fn inside() -> Result<(), String> {
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
    #[repr(C)]
    struct Header {
        version: u32,
        pid: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }
    const VERSION_3: u32 = 0x2008_0522;
    let header = Header { version: VERSION_3, pid: 0 };
    let data = [Data { effective: 0, permitted: 0, inheritable: 0 }; 2];
    // SAFETY: capset with a v3 header and its two data words.
    if unsafe { libc::syscall(libc::SYS_capset, &header as *const Header, data.as_ptr()) } != 0 {
        return Err(format!("couldn't drop the browser's capabilities: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

fn write(path: &str, what: &str) -> Result<(), String> {
    std::fs::write(path, what).map_err(|error| format!("couldn't map the browser's user ({path}): {error}"))
}
