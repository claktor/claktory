//! How many files factoryd may have open (#490 risk 5). launchd starts a
//! Mac's processes with a soft limit of 256 open files, and systemd with
//! 1024: a Claktory with many agents, their connections, tasks and watches
//! ran out under load, and so did agents' commands, which inherit it. So
//! the backend and the supervisor raise their soft limit to the hard one,
//! at most `MOST`, at start; agents' commands inherit the raised limit.

/// The most open files asked for: plenty for any Claktory, and what a
/// command that walks every possible descriptor still does quickly.
pub const MOST: u64 = 65_536;

/// What macOS took for the most before it said otherwise (OPEN_MAX).
#[cfg(unix)]
const OPEN_MAX: u64 = 10_240;

/// The kernel's own most per process, where it says (macOS).
#[cfg(target_os = "macos")]
fn kernel_most() -> Option<u64> {
    let mut most: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: sysctlbyname writes at most `size` bytes into `most`.
    let read = unsafe {
        libc::sysctlbyname(c"kern.maxfilesperproc".as_ptr(), (&raw mut most).cast(), &raw mut size, std::ptr::null_mut(), 0)
    };
    (read == 0 && most > 0).then(|| most.unsigned_abs().into())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn kernel_most() -> Option<u64> {
    None
}

/// This process's soft and hard limits on open files.
#[cfg(unix)]
pub fn limits() -> Option<(u64, u64)> {
    let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: getrlimit only writes `limit`.
    (unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) } == 0).then_some((limit.rlim_cur, limit.rlim_max))
}

#[cfg(unix)]
fn set(soft: u64, hard: u64) -> bool {
    let limit = libc::rlimit { rlim_cur: soft, rlim_max: hard };
    // SAFETY: setrlimit only reads `limit`.
    unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit) == 0 }
}

/// Raises this process's soft limit on open files to its hard limit, at
/// most `MOST` (and the kernel's most, where it says); never lowers it.
/// Says what it was and what it is now, once, in the log. The limits
/// before and after; None when they can't be read.
#[cfg(unix)]
pub fn raise() -> Option<(u64, u64)> {
    let Some((before, hard)) = limits() else {
        tracing::warn!(error = %std::io::Error::last_os_error(), "could not read the open-files limit");
        return None;
    };
    let wanted = hard.min(MOST).min(kernel_most().unwrap_or(u64::MAX));
    if before >= wanted {
        tracing::info!(open_files = before, hard, "open-files limit");
        return Some((before, before));
    }
    // macOS has refused more than OPEN_MAX in some versions: then that.
    let after = [wanted, wanted.min(OPEN_MAX)].into_iter().find(|&soft| soft > before && set(soft, hard)).unwrap_or(before);
    if after == before {
        tracing::warn!(open_files = before, hard, error = %std::io::Error::last_os_error(), "could not raise the open-files limit");
    } else {
        tracing::info!(open_files = after, was = before, hard, "raised the open-files limit");
    }
    Some((before, after))
}

/// Windows has no such limit: a process may hold millions of handles. None,
/// as there's nothing to read.
#[cfg(windows)]
pub fn limits() -> Option<(u64, u64)> {
    None
}

/// Nothing to raise on Windows (`limits`); None, said once in the log.
#[cfg(windows)]
pub fn raise() -> Option<(u64, u64)> {
    tracing::info!("no open-files limit to raise on Windows");
    None
}
