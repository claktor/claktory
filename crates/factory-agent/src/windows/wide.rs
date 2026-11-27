//! Small helpers for calling Win32: UTF-16 strings, handles, errors.

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};

/// `text` as a NUL-terminated UTF-16 string. A NUL inside it is refused:
/// Windows would read only up to it.
pub fn wide(text: impl AsRef<OsStr>) -> std::io::Result<Vec<u16>> {
    let mut units: Vec<u16> = text.as_ref().encode_wide().collect();
    if units.contains(&0) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "a string for Windows holds a NUL"));
    }
    units.push(0);
    Ok(units)
}

/// A NUL-terminated UTF-16 string Windows gave, as an OsString.
///
/// # Safety
/// `pointer` is null or points at a NUL-terminated UTF-16 string.
pub unsafe fn from_wide_ptr(pointer: *const u16) -> OsString {
    if pointer.is_null() {
        return OsString::new();
    }
    let mut length = 0;
    // SAFETY: the string ends with a NUL, per the caller.
    while unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` units were just read.
    OsString::from_wide(unsafe { std::slice::from_raw_parts(pointer, length) })
}

/// A handle Windows returned, owned; the last error when it's null or
/// INVALID_HANDLE_VALUE (functions differ in which they return).
pub fn owned(handle: HANDLE) -> std::io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a fresh handle we own and nothing else closes.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// The last error when a BOOL-returning function failed.
pub fn check(result: i32) -> std::io::Result<()> {
    if result == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}
