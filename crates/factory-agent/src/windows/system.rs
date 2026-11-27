//! What the system says about time and processes: when the computer
//! started, when a process started, how one ended, random bytes, and who
//! this user is (for the protected DACLs Claktory's objects get).

use std::os::windows::io::{AsRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::process::ExitStatus;

use windows_sys::Win32::Foundation::{FILETIME, HLOCAL, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::SystemInformation::{GetSystemDirectoryW, GetSystemTimeAsFileTime, GetTickCount64};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetProcessTimes, GetThreadTimes, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};

use super::wide::{check, from_wide_ptr, owned, wide};

/// 100-ns intervals between 1601 (FILETIME's epoch) and 1970.
const FILETIME_UNIX_OFFSET: u64 = 116_444_736_000_000_000;

fn filetime(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Now, in ms since 1970.
fn now_ms() -> u64 {
    let mut now = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    // SAFETY: writes one FILETIME.
    unsafe { GetSystemTimeAsFileTime(&raw mut now) };
    filetime(now).saturating_sub(FILETIME_UNIX_OFFSET) / 10_000
}

/// When this computer last started, in ms since 1970: now less the time
/// since it started (which counts time asleep). Fast Startup's "shut down"
/// hibernates the system rather than restarting it, so it doesn't move
/// this; the processes it ends read as gone, not as ended by a restart.
pub fn boot_time_ms() -> Option<u64> {
    // SAFETY: no arguments.
    let up = unsafe { GetTickCount64() };
    now_ms().checked_sub(up).filter(|boot| *boot > 0)
}

/// A process, opened only to ask about it.
pub fn open_process(pid: u32) -> std::io::Result<OwnedHandle> {
    // SAFETY: plain call; the handle is checked and owned.
    owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })
}

/// When a process started, in FILETIME units: with its pid, what tells it
/// from a later process given the same pid.
pub fn creation_time(process: &OwnedHandle) -> std::io::Result<u64> {
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: four FILETIMEs to write.
    check(unsafe { GetProcessTimes(process.as_raw_handle(), &raw mut created, &raw mut exited, &raw mut kernel, &raw mut user) })?;
    Ok(filetime(created))
}

/// The processor time the calling thread has spent so far, user and
/// kernel: what CLOCK_THREAD_CPUTIME_ID gives elsewhere.
pub fn thread_cpu_time() -> std::io::Result<std::time::Duration> {
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the pseudo handle of this thread and four FILETIMEs to write.
    check(unsafe { GetThreadTimes(GetCurrentThread(), &raw mut created, &raw mut exited, &raw mut kernel, &raw mut user) })?;
    // 100 ns units.
    Ok(std::time::Duration::from_nanos((filetime(kernel) + filetime(user)) * 100))
}

/// The file a process runs, absolute; empty when it can't be told.
pub fn image_path(process: &OwnedHandle) -> String {
    let mut buffer = vec![0u16; 32_768];
    let mut size = buffer.len() as u32;
    // SAFETY: `size` units to write, and it says how many were.
    let done = unsafe { QueryFullProcessImageNameW(process.as_raw_handle(), 0, buffer.as_mut_ptr(), &raw mut size) };
    if done == 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..size as usize])
}

/// Whether `image` (a full path, as `image_path` reads it) is Windows'
/// console host, `conhost.exe` in the system folder. The console a command
/// is started with brings one into its job, and it outlives the command's
/// last process by a moment, so it never counts as the command running.
pub fn is_console_host(image: &str) -> bool {
    static HOST: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let host = HOST.get_or_init(|| {
        let mut buffer = vec![0u16; 1024];
        // SAFETY: `buffer.len()` units to write; it returns how many were.
        let size = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        (size > 0 && size < buffer.len()).then(|| format!("{}\\conhost.exe", String::from_utf16_lossy(&buffer[..size])))
    });
    host.as_deref().is_some_and(|host| host.eq_ignore_ascii_case(image))
}

/// The exit code a command gets when Claktory stops its job: an
/// application-defined NTSTATUS (the customer bit set), so no program's own
/// exit code reads as it.
pub const STOPPED_CODE: u32 = 0xE04B_0001;

/// How a process that ended other than by exiting ended, as a name, the way
/// a signal's name says it elsewhere; None for an ordinary exit code.
pub fn ended_by(status: &ExitStatus) -> Option<&'static str> {
    // Always Some on Windows; the code is a u32 there.
    let code = status.code()? as u32;
    Some(match code {
        STOPPED_CODE => "stopped",
        0xC000_0005 => "access violation",
        0xC000_001D => "illegal instruction",
        0xC000_008E | 0xC000_0090 | 0xC000_0094 => "arithmetic error",
        0xC000_00FD => "stack overflow",
        0xC000_013A => "Ctrl+C",
        0xC000_0409 => "fast fail",
        0xC000_0374 => "heap corruption",
        0xC000_0135 => "a DLL wasn't found",
        0xC000_0142 => "a DLL failed to start",
        _ => return None,
    })
}

/// An ExitStatus from the code Windows gave.
pub fn exit_status(code: u32) -> ExitStatus {
    ExitStatus::from_raw(code)
}

/// `count` random bytes from the system's generator.
pub fn random_bytes<const N: usize>() -> std::io::Result<[u8; N]> {
    let mut bytes = [0u8; N];
    // SAFETY: N bytes to write; the system-preferred generator needs no handle.
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), N as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if status != 0 {
        return Err(std::io::Error::other(format!("the system's random numbers failed (NTSTATUS {status:#x})")));
    }
    Ok(bytes)
}

/// This process's user, as a SID string ("S-1-5-21-…"). Read once.
pub fn user_sid() -> std::io::Result<String> {
    static SID: std::sync::OnceLock<Result<String, String>> = std::sync::OnceLock::new();
    SID.get_or_init(|| read_user_sid().map_err(|error| error.to_string())).clone().map_err(std::io::Error::other)
}

fn read_user_sid() -> std::io::Result<String> {
    let mut token = std::ptr::null_mut();
    // SAFETY: the current process's pseudo-handle; the token is checked and owned.
    check(unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) })?;
    let token = owned(token)?;
    let mut size = 0u32;
    // SAFETY: asks only for the size.
    unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, std::ptr::null_mut(), 0, &raw mut size) };
    // u64s, so the TOKEN_USER at its start is aligned.
    let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
    // SAFETY: `size` bytes to write.
    check(unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, buffer.as_mut_ptr().cast(), size, &raw mut size) })?;
    // SAFETY: the buffer now starts with a TOKEN_USER.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut text = std::ptr::null_mut();
    // SAFETY: the SID inside the buffer; the string is LocalFree'd below.
    check(unsafe { ConvertSidToStringSidW(user.User.Sid, &raw mut text) })?;
    // SAFETY: a NUL-terminated string Windows allocated.
    let sid = unsafe { from_wide_ptr(text) }.to_string_lossy().into_owned();
    // SAFETY: allocated by ConvertSidToStringSidW.
    unsafe { LocalFree(text as HLOCAL) };
    Ok(sid)
}

/// A security descriptor owned by this user whose DACL is protected
/// (inherits nothing) and gives full access to this user and SYSTEM only:
/// what Claktory's job objects get (Fugo 44974), and its private files and
/// folders. Kept alive while something is created with it.
pub struct Protected {
    descriptor: PSECURITY_DESCRIPTOR,
}

/// SYSTEM's SID.
const SYSTEM: &str = "S-1-5-18";

// SAFETY: the descriptor is plain memory, only read once made.
unsafe impl Send for Protected {}
unsafe impl Sync for Protected {}

impl Protected {
    /// For a file or a job: this user and SYSTEM. The owner is set to the
    /// user explicitly, so it's the user even from an elevated token (whose
    /// default owner is Administrators), as `is_private` expects.
    pub fn for_user() -> std::io::Result<Protected> {
        let sid = user_sid()?;
        Protected::from_sddl(&format!("O:{sid}D:P(A;;GA;;;{sid})(A;;GA;;;SY)"))
    }

    /// For a folder: the same, inherited by everything made inside it
    /// (OICI), so a file a library creates there with no DACL of its own
    /// (SQLite's, a log) is private too (Fugo 45257, LOW 2).
    pub fn for_folder() -> std::io::Result<Protected> {
        let sid = user_sid()?;
        Protected::from_sddl(&format!("O:{sid}D:P(A;OICI;GA;;;{sid})(A;OICI;GA;;;SY)"))
    }

    fn from_sddl(sddl: &str) -> std::io::Result<Protected> {
        let text = wide(sddl)?;
        let mut descriptor = std::ptr::null_mut();
        // SAFETY: a NUL-terminated string; the descriptor is LocalFree'd on drop.
        check(unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(), SDDL_REVISION_1, &raw mut descriptor, std::ptr::null_mut())
        })?;
        Ok(Protected { descriptor })
    }

    /// Attributes to create something with it, not inherited by children.
    pub fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor,
            bInheritHandle: 0,
        }
    }
}

impl Drop for Protected {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
        unsafe { LocalFree(self.descriptor as HLOCAL) };
    }
}

/// "Tue 14:05": the weekday and time of `at` (ms since 1970) in this
/// computer's time zone, as `accounts::local_time` says it elsewhere.
pub fn local_time(at: u64) -> Option<String> {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let units = at.checked_mul(10_000)?.checked_add(FILETIME_UNIX_OFFSET)?;
    let time = FILETIME { dwLowDateTime: units as u32, dwHighDateTime: (units >> 32) as u32 };
    // SAFETY: all-zero SYSTEMTIMEs are valid to write over.
    let (mut universal, mut local): (SYSTEMTIME, SYSTEMTIME) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    // SAFETY: reads one FILETIME, writes one SYSTEMTIME.
    check(unsafe { FileTimeToSystemTime(&raw const time, &raw mut universal) }).ok()?;
    // SAFETY: a null time zone means the current one; writes one SYSTEMTIME.
    check(unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &raw const universal, &raw mut local) }).ok()?;
    Some(format!("{} {:02}:{:02}", DAYS.get(usize::from(local.wDayOfWeek))?, local.wHour, local.wMinute))
}

/// Windows' version, "10.0.26100", as Node's `os.release()` says it (and
/// whatever the program's manifest claims, which GetVersionEx would heed).
pub fn kernel_release() -> String {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    // SAFETY: an all-zero OSVERSIONINFOW is valid once its size is set.
    let mut info: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
    info.dwOSVersionInfoSize = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    // SAFETY: writes the structure whose size it was told.
    if unsafe { RtlGetVersion(&raw mut info) } != 0 {
        return String::new();
    }
    format!("{}.{}.{}", info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber)
}

/// Ends one process at once, as SIGKILL does: what the guard does to a
/// program that asks to act as someone else. Whether it could.
pub fn terminate_process(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: plain call; the handle is checked and owned.
    let Ok(process) = owned(unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) }) else { return false };
    // SAFETY: an open handle with PROCESS_TERMINATE.
    unsafe { TerminateProcess(process.as_raw_handle(), STOPPED_CODE) != 0 }
}

/// Who owns something and who its DACL lets in.
struct Security {
    owner: String,
    /// The DACL inherits nothing.
    protected: bool,
    /// The SIDs of its ACEs, each allowing; None if any ACE isn't an
    /// allowing one.
    allowed: Option<Vec<String>>,
}

impl Security {
    /// Owned by this user, protected, and letting in this user and SYSTEM
    /// alone: what `Protected` makes.
    fn private(&self) -> std::io::Result<bool> {
        let user = user_sid()?;
        let Some(allowed) = &self.allowed else { return Ok(false) };
        let only_ours = allowed.len() == 2 && allowed.contains(&user) && allowed.iter().any(|sid| sid == SYSTEM);
        Ok(self.owner == user && self.protected && only_ours)
    }

    /// Reads it out of a descriptor Windows allocated, and frees it.
    ///
    /// # Safety
    /// `descriptor` came from GetSecurityInfo or GetNamedSecurityInfoW with
    /// OWNER and DACL asked for, and `owner` and `dacl` point into it.
    unsafe fn take(
        descriptor: PSECURITY_DESCRIPTOR,
        owner: windows_sys::Win32::Security::PSID,
        dacl: *mut windows_sys::Win32::Security::ACL,
    ) -> std::io::Result<Security> {
        use windows_sys::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION, AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorControl,
            SE_DACL_PROTECTED,
        };
        const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;
        let read = (|| {
            let owner = sid_string(owner)?;
            let (mut control, mut revision) = (0u16, 0u32);
            // SAFETY: a valid descriptor, per the caller.
            check(unsafe { GetSecurityDescriptorControl(descriptor, &raw mut control, &raw mut revision) })?;
            let protected = control & SE_DACL_PROTECTED != 0;
            if dacl.is_null() {
                // No DACL at all lets everyone in.
                return Ok(Security { owner, protected, allowed: None });
            }
            // SAFETY: an all-zero size structure is valid to write over.
            let mut size: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: the DACL inside the descriptor, and the structure's size.
            check(unsafe {
                GetAclInformation(dacl, (&raw mut size).cast(), std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation)
            })?;
            let mut allowed = Vec::new();
            for index in 0..size.AceCount {
                let mut ace = std::ptr::null_mut();
                // SAFETY: an index below the count.
                check(unsafe { GetAce(dacl, index, &raw mut ace) })?;
                // SAFETY: every ACE starts with its header.
                let header = unsafe { &*ace.cast::<windows_sys::Win32::Security::ACE_HEADER>() };
                if header.AceType != ACCESS_ALLOWED_ACE_TYPE {
                    return Ok(Security { owner, protected, allowed: None });
                }
                let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
                // SAFETY: an allowing ACE's SID starts at SidStart.
                allowed.push(sid_string(unsafe { (&raw mut (*ace).SidStart).cast() })?);
            }
            Ok(Security { owner, protected, allowed: Some(allowed) })
        })();
        // SAFETY: allocated by the call that gave it, freed once.
        unsafe { LocalFree(descriptor as HLOCAL) };
        read
    }
}

fn sid_string(sid: windows_sys::Win32::Security::PSID) -> std::io::Result<String> {
    if sid.is_null() {
        return Ok(String::new());
    }
    let mut text = std::ptr::null_mut();
    // SAFETY: a valid SID; the string is LocalFree'd below.
    check(unsafe { ConvertSidToStringSidW(sid, &raw mut text) })?;
    // SAFETY: a NUL-terminated string Windows allocated.
    let sid = unsafe { from_wide_ptr(text) }.to_string_lossy().into_owned();
    // SAFETY: allocated by ConvertSidToStringSidW.
    unsafe { LocalFree(text as HLOCAL) };
    Ok(sid)
}

const OWNER_AND_DACL: u32 =
    windows_sys::Win32::Security::OWNER_SECURITY_INFORMATION | windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

/// Whether a kernel object (a job) is owned by this user with the private
/// DACL `Protected` gives: how a job found again by its name is told from
/// one another account made under it (Fugo 45257 (a)).
pub fn is_private_object(handle: &OwnedHandle) -> std::io::Result<bool> {
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    let (mut owner, mut dacl, mut descriptor) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: an open handle; the descriptor is freed by Security::take.
    let result = unsafe {
        GetSecurityInfo(
            handle.as_raw_handle(),
            SE_KERNEL_OBJECT,
            OWNER_AND_DACL,
            &raw mut owner,
            std::ptr::null_mut(),
            &raw mut dacl,
            std::ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result as i32));
    }
    // SAFETY: as GetSecurityInfo gave them.
    unsafe { Security::take(descriptor, owner, dacl) }?.private()
}

/// A file's or folder's owner and whether its DACL is the private one.
fn path_security(path: &std::path::Path) -> std::io::Result<Security> {
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
    let name = wide(path)?;
    let (mut owner, mut dacl, mut descriptor) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: a NUL-terminated path; the descriptor is freed by Security::take.
    let result = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_AND_DACL,
            &raw mut owner,
            std::ptr::null_mut(),
            &raw mut dacl,
            std::ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result as i32));
    }
    // SAFETY: as GetNamedSecurityInfoW gave them.
    unsafe { Security::take(descriptor, owner, dacl) }
}

/// Whether a file or folder is private already (`private::create_dir_all`
/// doesn't rewrite a whole tree's DACLs at every start when it is).
pub fn is_private_path(path: &std::path::Path) -> std::io::Result<bool> {
    path_security(path)?.private()
}

/// Whether a file's or folder's DACL lets in anyone but this user and
/// SYSTEM (or has no DACL, or a denying ACE Claktory never writes).
pub fn open_to_others(path: &std::path::Path) -> std::io::Result<bool> {
    let user = user_sid()?;
    Ok(match path_security(path)?.allowed {
        None => true,
        Some(allowed) => allowed.iter().any(|sid| *sid != user && sid != SYSTEM),
    })
}

/// Whether this user owns a file or folder.
pub fn owned_by_user(path: &std::path::Path) -> std::io::Result<bool> {
    Ok(path_security(path)?.owner == user_sid()?)
}

/// Hex, for names made of random bytes.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_computer_started_in_the_past() {
        let boot = boot_time_ms().expect("a boot time");
        assert!(boot < now_ms());
    }

    #[test]
    fn this_process_has_a_creation_time_and_a_path() {
        let me = open_process(std::process::id()).expect("opens itself");
        assert!(creation_time(&me).expect("a creation time") > FILETIME_UNIX_OFFSET);
        assert!(image_path(&me).to_lowercase().ends_with(".exe"));
        let before = thread_cpu_time().expect("this thread's time");
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(50);
        let mut spin = 0u64;
        while std::time::Instant::now() < deadline {
            spin = std::hint::black_box(spin.wrapping_add(1));
        }
        assert!(thread_cpu_time().unwrap() > before, "busy for 50 ms, it counts");
    }

    #[test]
    fn stopping_reads_as_stopped_and_exit_codes_as_none() {
        assert_eq!(ended_by(&exit_status(STOPPED_CODE)), Some("stopped"));
        assert_eq!(ended_by(&exit_status(0xC000_0005)), Some("access violation"));
        assert_eq!(ended_by(&exit_status(0)), None);
        assert_eq!(ended_by(&exit_status(1)), None);
    }

    #[test]
    fn local_time_and_the_release_read_as_on_other_systems() {
        let text = local_time(1_764_000_000_000).expect("a local time");
        assert_eq!(text.len(), 9, "{text}");
        assert!(["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].contains(&&text[..3]));
        assert!(kernel_release().starts_with("10.0."), "{}", kernel_release());
        assert!(!terminate_process(u32::MAX - 3));
    }

    #[test]
    fn random_names_differ() {
        assert_ne!(random_bytes::<16>().unwrap(), random_bytes::<16>().unwrap());
    }

    #[test]
    fn the_user_is_a_sid_and_the_descriptors_parse() {
        assert!(user_sid().unwrap().starts_with("S-1-"));
        Protected::for_user().expect("a protected descriptor");
        Protected::for_folder().expect("a protected folder's descriptor");
    }

    #[test]
    fn a_temporary_folder_is_not_private_and_this_user_owns_it() {
        let folder = std::env::temp_dir();
        assert!(!is_private_path(&folder).unwrap());
        assert!(owned_by_user(&folder).unwrap());
    }
}
