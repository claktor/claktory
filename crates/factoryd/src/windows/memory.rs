//! The vault's key in memory on Windows (`vault::memory`): a page of its
//! own, locked into memory so it's never written to the page file
//! (VirtualLock, as mlock), and left out of any crash report Windows makes
//! of the process (WerRegisterExcludedMemoryBlock, as MADV_DONTDUMP).

use std::ptr::NonNull;

use windows_sys::Win32::System::ErrorReporting::{WerRegisterExcludedMemoryBlock, WerUnregisterExcludedMemoryBlock};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualLock, VirtualUnlock,
};
use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};

/// A page of this process's memory, zeroed, locked and out of crash
/// reports where Windows allows; None if no page can be had.
pub fn page() -> Option<(NonNull<u8>, usize)> {
    // SAFETY: GetSystemInfo fills the structure it's given.
    let size = unsafe {
        let mut info: SYSTEM_INFO = std::mem::zeroed();
        GetSystemInfo(&raw mut info);
        info.dwPageSize as usize
    };
    let size = if size >= 32 { size } else { 4096 };
    // SAFETY: a fresh committed region of `size` bytes, checked below.
    let page = NonNull::new(unsafe { VirtualAlloc(std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) }.cast::<u8>())?;
    // SAFETY: `page` is `size` bytes this process owns.
    unsafe {
        if VirtualLock(page.as_ptr().cast(), size) == 0 {
            tracing::warn!(error = %std::io::Error::last_os_error(), "could not keep the vault's key out of the page file");
        }
        if WerRegisterExcludedMemoryBlock(page.as_ptr().cast(), size as u32) < 0 {
            tracing::warn!("could not keep the vault's key out of crash reports");
        }
    }
    Some((page, size))
}

/// Gives back a page `page` gave, once its bytes are wiped.
///
/// # Safety
/// `page` and `size` are what `page` returned, not given back before.
pub unsafe fn free(page: NonNull<u8>, size: usize) {
    // SAFETY: as the caller says.
    unsafe {
        WerUnregisterExcludedMemoryBlock(page.as_ptr().cast());
        VirtualUnlock(page.as_ptr().cast(), size);
        VirtualFree(page.as_ptr().cast(), 0, MEM_RELEASE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_had_written_and_given_back() {
        let (page, size) = page().expect("a page");
        assert!(size >= 4096);
        // SAFETY: the page is `size` bytes, ours until `free`.
        unsafe {
            assert!(std::slice::from_raw_parts(page.as_ptr(), size).iter().all(|byte| *byte == 0), "zeroed");
            page.as_ptr().write(7);
            assert_eq!(page.as_ptr().read(), 7);
            free(page, size);
        }
    }
}
