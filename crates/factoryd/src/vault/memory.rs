//! Keeping the vault's key out of reach while it's in memory: in a page of
//! its own that's never swapped out (mlock) or written into a core dump, and
//! a process that doesn't dump core and, on Linux, can't be read by other
//! processes of the same user, agents included (PR_SET_DUMPABLE 0).

use std::ptr::NonNull;

use zeroize::Zeroize;

/// A 32-byte key in a locked page of its own, wiped when dropped.
pub(crate) struct Locked {
    page: NonNull<u8>,
    size: usize,
}

// The page is owned, like a Box.
unsafe impl Send for Locked {}
unsafe impl Sync for Locked {}

impl Locked {
    /// `key` in a locked page; None if no page can be had.
    pub(crate) fn new(key: &[u8; 32]) -> Option<Locked> {
        // SAFETY: an anonymous private mapping of one page, checked below.
        let size = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).ok().filter(|size| *size >= 32).unwrap_or(4096);
        let page = unsafe {
            libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANON, -1, 0)
        };
        if page == libc::MAP_FAILED {
            return None;
        }
        let page = NonNull::new(page.cast::<u8>())?;
        // SAFETY: `page` is `size` bytes we own.
        unsafe {
            if libc::mlock(page.as_ptr().cast(), size) != 0 {
                tracing::warn!(error = %std::io::Error::last_os_error(), "could not keep the vault's key out of swap");
            }
            #[cfg(target_os = "linux")]
            libc::madvise(page.as_ptr().cast(), size, libc::MADV_DONTDUMP);
            std::ptr::copy_nonoverlapping(key.as_ptr(), page.as_ptr(), 32);
        }
        Some(Locked { page, size })
    }
}

impl std::ops::Deref for Locked {
    type Target = [u8; 32];

    fn deref(&self) -> &[u8; 32] {
        // SAFETY: the page holds the 32 bytes written in `new`.
        unsafe { &*self.page.as_ptr().cast::<[u8; 32]>() }
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        // SAFETY: the page is ours until munmap.
        unsafe {
            std::slice::from_raw_parts_mut(self.page.as_ptr(), self.size).zeroize();
            libc::munlock(self.page.as_ptr().cast(), self.size);
            libc::munmap(self.page.as_ptr().cast(), self.size);
        }
    }
}

/// Makes this process dump no core, and on Linux keeps other processes of
/// the same user from reading its memory. For the backend and the
/// supervisor, which hold the vault's key. Children are unaffected but for
/// the core size, which they inherit and may raise again.
pub fn harden() {
    // SAFETY: plain system calls on this process.
    unsafe {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if libc::getrlimit(libc::RLIMIT_CORE, &mut limit) == 0 {
            limit.rlim_cur = 0;
            libc::setrlimit(libc::RLIMIT_CORE, &limit);
        }
        #[cfg(target_os = "linux")]
        if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 {
            tracing::warn!(error = %std::io::Error::last_os_error(), "could not keep other processes from reading Claktory's memory");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_locked_key_reads_back() {
        let key = [7u8; 32];
        let locked = Locked::new(&key).expect("a page");
        assert_eq!(*locked, key);
    }
}
