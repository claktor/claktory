//! Files on Windows: what the backend reads from `std::os::unix::fs` and
//! `statvfs` on a Mac or Linux, by the Win32 calls that give the same.

use std::fs::{File, Metadata};
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::time::UNIX_EPOCH;

use factory_agent::windows::wide::{check, wide};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, GetDiskFreeSpaceExW, GetFileInformationByHandle,
};

/// What `std::os::unix::fs::MetadataExt` gives the backend, from what
/// Windows keeps in a file's metadata, under the same names, so a file that
/// reads them needs only its `use` made per platform.
pub trait MetadataExt {
    /// 512-byte blocks it takes: its length rounded up to the 4 KiB cluster
    /// NTFS uses unless told otherwise. A sparse or compressed file takes
    /// less than this says.
    fn blocks(&self) -> u64;
    /// When it was last written, in seconds since 1970.
    fn mtime(&self) -> i64;
}

impl MetadataExt for Metadata {
    fn blocks(&self) -> u64 {
        const CLUSTER: u64 = 4096;
        self.file_size().div_ceil(CLUSTER) * (CLUSTER / 512)
    }

    fn mtime(&self) -> i64 {
        self.modified().ok().and_then(|at| at.duration_since(UNIX_EPOCH).ok()).map_or(0, |since| since.as_secs() as i64)
    }
}

/// What `std::os::unix::fs::FileExt` gives the backend: reads and writes
/// at an offset. Unlike pread and pwrite these move the file's position
/// too, so a file used through them is used only through them.
pub trait FileExt {
    /// Reads exactly `buf.len()` bytes from `offset`.
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<()>;
    /// Writes all of `buf` at `offset`.
    fn write_all_at(&self, buf: &[u8], offset: u64) -> std::io::Result<()>;
}

impl FileExt for File {
    fn read_exact_at(&self, mut buf: &mut [u8], mut offset: u64) -> std::io::Result<()> {
        use std::os::windows::fs::FileExt as _;
        while !buf.is_empty() {
            match self.seek_read(buf, offset) {
                Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                Ok(read) => {
                    buf = &mut buf[read..];
                    offset += read as u64;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn write_all_at(&self, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
        use std::os::windows::fs::FileExt as _;
        while !buf.is_empty() {
            match self.seek_write(buf, offset) {
                Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
                Ok(written) => {
                    buf = &buf[written..];
                    offset += written as u64;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }
}

/// Which file a name is, as a device and inode number say elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The volume's serial number (st_dev).
    pub volume: u64,
    /// The file's index on its volume (st_ino).
    pub index: u64,
    /// How many names it has (st_nlink).
    pub links: u64,
}

/// The identity of what `path` names, not of what a link there points to
/// (as lstat).
pub fn identity(path: &Path) -> std::io::Result<Identity> {
    let file =
        std::fs::OpenOptions::new().access_mode(0).custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
    identity_of(&file)
}

/// The identity of an open file (or a standard handle that's one).
pub fn identity_of(file: &impl AsRawHandle) -> std::io::Result<Identity> {
    // SAFETY: an all-zero structure is valid to write over.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: an open handle and a structure of the type asked for.
    check(unsafe { GetFileInformationByHandle(file.as_raw_handle(), &raw mut info) })?;
    Ok(Identity {
        volume: u64::from(info.dwVolumeSerialNumber),
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        links: u64::from(info.nNumberOfLinks),
    })
}

/// Whether an open file only appends (FILE_APPEND_DATA without
/// FILE_WRITE_DATA): what O_APPEND is elsewhere, so whatever writes through
/// it writes at the end, however long the file is meanwhile.
pub fn appends_only(file: &impl AsRawHandle) -> std::io::Result<bool> {
    use windows_sys::Wdk::Storage::FileSystem::{FILE_ACCESS_INFORMATION, FileAccessInformation, NtQueryInformationFile};
    use windows_sys::Win32::Storage::FileSystem::{FILE_APPEND_DATA, FILE_WRITE_DATA};
    // SAFETY: all-zero structures are valid to write over.
    let (mut status, mut access): (windows_sys::Win32::System::IO::IO_STATUS_BLOCK, FILE_ACCESS_INFORMATION) =
        unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    // SAFETY: an open handle and a structure of the class asked for.
    let result = unsafe {
        NtQueryInformationFile(
            file.as_raw_handle(),
            &raw mut status,
            (&raw mut access).cast(),
            size_of::<FILE_ACCESS_INFORMATION>() as u32,
            FileAccessInformation,
        )
    };
    if result < 0 {
        return Err(std::io::Error::other(format!("couldn't read how a file is open (NTSTATUS {result:#x})")));
    }
    Ok(access.AccessFlags & FILE_APPEND_DATA != 0 && access.AccessFlags & FILE_WRITE_DATA == 0)
}

/// FILE_ATTRIBUTE_REPARSE_POINT: a junction, a symbolic link, a mount point
/// (or a cloud file's placeholder).
const REPARSE_POINT: u32 = 0x400;
/// FILE_ATTRIBUTE_DIRECTORY.
const DIRECTORY: u32 = 0x10;

/// Opens the file at `path` (absolute, on a drive) to read, following no
/// junction, symbolic link or mount point anywhere in it, even one put
/// there while it's opened: what openat with O_NOFOLLOW from `/` does
/// elsewhere. Each folder on the way is opened as itself (not what it
/// points to), refused if it's a reparse point, and held open without
/// FILE_SHARE_DELETE until the file is open, so it can't be renamed or
/// swapped for a link meanwhile; the next is opened through the ones held.
pub fn open_walked(path: &Path) -> std::io::Result<File> {
    use std::path::{Component, Prefix};
    let mut components = path.components();
    let (Some(Component::Prefix(prefix)), Some(Component::RootDir)) = (components.next(), components.next()) else {
        return Err(refused(format!("{} isn't a full path on a drive", path.display())));
    };
    if !matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) {
        return Err(refused(format!("{} isn't on a drive of this computer", path.display())));
    }
    let names: Vec<_> = components.collect();
    let Some((file, folders)) = names.split_last() else { return Err(refused(format!("{} isn't a file's path", path.display()))) };
    let mut so_far = std::path::PathBuf::from(prefix.as_os_str());
    so_far.push(std::path::MAIN_SEPARATOR_STR);
    let mut held = Vec::with_capacity(folders.len());
    for name in folders {
        let Component::Normal(name) = name else { return Err(refused(format!("{} isn't a plain path", path.display()))) };
        so_far.push(name);
        held.push(open_folder(&so_far, false)?);
    }
    let Component::Normal(name) = file else { return Err(refused(format!("{} isn't a plain path", path.display()))) };
    so_far.push(name);
    let opened = open_file(&so_far)?;
    drop(held);
    Ok(opened)
}

/// Why a walk refused a path: a link on the way (`is_link` tells), or a
/// path it doesn't take.
fn refused(why: String) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, why)
}

/// Whether `error` is a walk's refusal of a junction, symbolic link or
/// mount point on the way.
pub fn is_link(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::InvalidInput && error.to_string().ends_with("is a link to somewhere else")
}

/// Opens the folder `path`, held so it can't be renamed, deleted or
/// swapped for a link while the handle's open (no FILE_SHARE_DELETE).
/// `follow`: whether `path` itself may be a link (the person's own choice
/// of folder); never when it's a step of a walk. Refused when it isn't a
/// folder.
pub fn open_folder(path: &Path, follow: bool) -> std::io::Result<File> {
    use windows_sys::Win32::Storage::FileSystem::{FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE};
    let flags = if follow { FILE_FLAG_BACKUP_SEMANTICS } else { FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT };
    // Listing access, not none: a handle opened only for its attributes
    // takes no part in sharing, so it wouldn't hold anything (the laptop's
    // test found a folder held that way renamed).
    let folder = std::fs::OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(flags)
        .open(path)?;
    let attributes = attributes_of(&folder)?;
    if attributes & REPARSE_POINT != 0 {
        return Err(refused(format!("{} is a link to somewhere else", path.display())));
    }
    if attributes & DIRECTORY == 0 {
        return Err(refused(format!("{} isn't a folder", path.display())));
    }
    Ok(folder)
}

/// Opens the file `path` to read, not following it if it's a link
/// (refused then), nor a folder.
pub fn open_file(path: &Path) -> std::io::Result<File> {
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;
    let opened = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        // A folder opened as a file is refused with ACCESS_DENIED: said so.
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::PermissionDenied if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) => {
                refused(format!("{} is a folder", path.display()))
            }
            _ => error,
        })?;
    let attributes = attributes_of(&opened)?;
    if attributes & REPARSE_POINT != 0 {
        return Err(refused(format!("{} is a link to somewhere else", path.display())));
    }
    if attributes & DIRECTORY != 0 {
        return Err(refused(format!("{} is a folder", path.display())));
    }
    Ok(opened)
}

/// Where an open file or folder is, its links resolved: `C:\Users\…`, as
/// GetFinalPathNameByHandleW gives it without its `\\?\`.
pub fn final_path(file: &File) -> std::io::Result<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::Storage::FileSystem::{FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW};
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: an open handle and `buffer.len()` units to write; it says how many it wrote.
    let size =
        unsafe { GetFinalPathNameByHandleW(file.as_raw_handle(), buffer.as_mut_ptr(), buffer.len() as u32, FILE_NAME_NORMALIZED) } as usize;
    if size == 0 || size >= buffer.len() {
        return Err(std::io::Error::last_os_error());
    }
    Ok(plain(std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..size]))))
}

/// `path` without the `\\?\` a resolved path starts with: `C:\Users\…`
/// and `\\server\share\…`, as people read and type them.
fn plain(path: std::path::PathBuf) -> std::path::PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(share) => std::path::PathBuf::from(format!(r"\\{share}")),
        None => text.strip_prefix(r"\\?\").map_or_else(|| path.clone(), std::path::PathBuf::from),
    }
}

/// `std::fs::canonicalize` as people read it: links and `..` resolved,
/// without the `\\?\` Windows puts first.
pub fn canonical(path: &Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::canonicalize(path).map(plain)
}

/// `path` with every 8.3 short name in it spelled out (`CLAKTO~3` →
/// `claktory-desktop`), as Windows keeps TEMP for some accounts; `path`
/// itself when it isn't there.
pub fn long_path(path: &Path) -> std::path::PathBuf {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
    let Ok(name) = wide(path) else { return path.to_path_buf() };
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: a NUL-terminated path and `buffer.len()` units to write; it says how many it wrote.
    let size = unsafe { GetLongPathNameW(name.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if size == 0 || size >= buffer.len() {
        return path.to_path_buf();
    }
    std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..size]))
}

/// Opens `path`, a file or a folder, only to set its times: Windows wants
/// write-attributes access for that, which `File::open` doesn't ask, and
/// opens a folder only with backup semantics. Shared every way, so a build
/// writing there meanwhile isn't refused.
pub fn open_to_date(path: &Path) -> std::io::Result<File> {
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES};
    std::fs::OpenOptions::new()
        .access_mode(FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

/// An open file's attributes (FILE_ATTRIBUTE_*).
fn attributes_of(file: &File) -> std::io::Result<u32> {
    // SAFETY: an all-zero structure is valid to write over.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: an open handle and a structure of the type asked for.
    check(unsafe { GetFileInformationByHandle(file.as_raw_handle(), &raw mut info) })?;
    Ok(info.dwFileAttributes)
}

/// Opens `path` as `options` say, never through a link at its end: what
/// `O_NOFOLLOW` does elsewhere. A symlink or junction there is opened
/// itself (FILE_FLAG_OPEN_REPARSE_POINT), then refused, and nothing is
/// made in its place.
pub fn open_no_follow(options: &mut std::fs::OpenOptions, path: &Path) -> std::io::Result<File> {
    let opened = options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
    if attributes_of(&opened)? & REPARSE_POINT != 0 {
        return Err(refused(format!("{} is a link to somewhere else", path.display())));
    }
    Ok(opened)
}

/// Opens the lock file at `path`, made private to this user (and SYSTEM) if
/// it isn't there, never through a link, and shared for reading and writing
/// but never deleting, so no one renames or deletes it while it's held. Its
/// lock is `File::try_lock` (LockFileEx, exclusive, failing at once), let go
/// when the file closes, as flock's is.
pub fn open_lock(path: &Path) -> std::io::Result<File> {
    use factory_agent::windows::system::Protected;
    use factory_agent::windows::wide::owned;
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GENERIC_READ, GENERIC_WRITE, GetLastError};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_ALWAYS};
    let protected = Protected::for_user()?;
    let attributes = protected.attributes();
    let name = wide(path)?;
    // SAFETY: a NUL-terminated path; the handle is checked and owned.
    let handle = unsafe {
        owned(CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &raw const attributes,
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        ))
    }?;
    // SAFETY: read right after the call that set it.
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let file = File::from(handle);
    let attributes = attributes_of(&file)?;
    if attributes & REPARSE_POINT != 0 {
        return Err(refused(format!("{} is a link to somewhere else", path.display())));
    }
    if attributes & DIRECTORY != 0 {
        return Err(refused(format!("{} is a folder", path.display())));
    }
    if existed {
        factory_agent::windows::private::protect(path)?;
    }
    Ok(file)
}

/// When what `path` names (not what a link there points to) last changed,
/// its contents or its metadata, in nanoseconds since 1970: the ctime a
/// change that keeps the modified time can't hide from.
pub fn changed_ns(path: &Path) -> std::io::Result<i64> {
    use windows_sys::Win32::Storage::FileSystem::{FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandleEx};
    /// 100-nanosecond intervals from 1601 to 1970.
    const FROM_1601: i64 = 116_444_736_000_000_000;
    let file =
        std::fs::OpenOptions::new().access_mode(0).custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).open(path)?;
    // SAFETY: an all-zero structure is valid to write over.
    let mut info: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
    // SAFETY: an open handle and a structure of the size and class asked for.
    check(unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            (&raw mut info).cast(),
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    })?;
    Ok(info.ChangeTime.saturating_sub(FROM_1601).saturating_mul(100))
}

/// Whether `path` is on a network disk (a mapped drive or a share), where
/// SQLite's WAL doesn't work.
pub fn on_network_disk(path: &Path) -> std::io::Result<bool> {
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;
    /// DRIVE_REMOTE.
    const REMOTE: u32 = 4;
    let root = wide(volume_root(path)?)?;
    // SAFETY: a NUL-terminated path.
    Ok(unsafe { GetDriveTypeW(root.as_ptr()) } == REMOTE)
}
/// Moves `from` to `to`, replacing what's there, and returns only once the
/// move is on the disk.
pub fn move_through(from: &Path, to: &Path) -> std::io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let (from, to) = (wide(from)?, wide(to)?);
    // SAFETY: two NUL-terminated paths.
    check(unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) })
}

/// Puts `from` in the place of `to`, written through to the disk, even
/// while `to` is a program that's running (whose file Windows won't write
/// over or delete, but lets be renamed): `to` is moved aside first, to a
/// name of its own beside it, which is returned for deleting once nothing
/// runs it; None when there was nothing at `to`. If `from` can't go in,
/// what was at `to` goes back.
pub fn replace(from: &Path, to: &Path) -> std::io::Result<Option<std::path::PathBuf>> {
    let move_file = move_through;
    let aside = if std::fs::symlink_metadata(to).is_ok() {
        let mut name = to.as_os_str().to_owned();
        name.push(format!(".replaced-{}", factory_agent::windows::system::hex(&factory_agent::windows::system::random_bytes::<6>()?)));
        let aside = std::path::PathBuf::from(name);
        move_file(to, &aside)?;
        Some(aside)
    } else {
        None
    };
    if let Err(error) = move_file(from, to) {
        if let Some(aside) = &aside {
            let _ = move_file(aside, to);
        }
        return Err(error);
    }
    Ok(aside)
}

/// Deletes what `replace` moved aside beside `path` and nothing runs now.
pub fn sweep_replaced(path: &Path) {
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else { return };
    let mut prefix = name.to_os_string();
    prefix.push(".replaced-");
    let prefix = prefix.to_string_lossy().into_owned();
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The root of the volume `path` is on (`C:\`, or the folder a volume is
/// mounted in): where it's mounted, as the last folder up on its device is
/// elsewhere.
pub fn volume_root(path: &Path) -> std::io::Result<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt as _;
    use windows_sys::Win32::Storage::FileSystem::GetVolumePathNameW;
    let name = wide(path)?;
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: a NUL-terminated path and `buffer.len()` units to write.
    check(unsafe { GetVolumePathNameW(name.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) })?;
    let end = buffer.iter().position(|unit| *unit == 0).unwrap_or(buffer.len());
    Ok(std::ffi::OsString::from_wide(&buffer[..end]).into())
}

/// A volume's size and room, in bytes, as statvfs gives them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Space {
    pub total: u64,
    /// Free on the volume.
    pub free: u64,
    /// Free for this user (less, under a quota).
    pub available: u64,
}

/// The size and room of the volume `path` is on.
pub fn space(path: &Path) -> std::io::Result<Space> {
    let name = wide(path)?;
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: a NUL-terminated path and three u64s to fill.
    check(unsafe { GetDiskFreeSpaceExW(name.as_ptr(), &raw mut available, &raw mut total, &raw mut free) })?;
    Ok(Space { total, free, available })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_tells_names_of_one_file_from_another_file() {
        let dir = std::env::temp_dir().join(format!("claktory-fs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b, c) = (dir.join("a"), dir.join("b"), dir.join("c"));
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&c, b"two").unwrap();
        std::fs::hard_link(&a, &b).unwrap();
        let (one, other, two) = (identity(&a).unwrap(), identity(&b).unwrap(), identity(&c).unwrap());
        assert_eq!(one, other);
        assert_eq!(one.links, 2);
        assert_ne!(one.index, two.index);
        assert_eq!((one.volume, two.links), (two.volume, 1));
        assert_eq!(identity(&dir).unwrap().volume, one.volume, "a folder has one too");
        assert_eq!(identity_of(&File::open(&c).unwrap()).unwrap(), two);
        assert!(appends_only(&std::fs::OpenOptions::new().append(true).open(&c).unwrap()).unwrap());
        assert!(!appends_only(&std::fs::OpenOptions::new().write(true).open(&c).unwrap()).unwrap());
        assert!(!appends_only(&File::open(&c).unwrap()).unwrap());
        let metadata = std::fs::metadata(&a).unwrap();
        assert_eq!(metadata.blocks(), 8, "three bytes take a cluster");
        assert!(metadata.mtime() > 1_700_000_000);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_walked_open_follows_no_junction_or_symbolic_link() {
        let dir = std::env::temp_dir().join(format!("claktory-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::write(dir.join("real").join("key"), b"secret").unwrap();
        let mut read = String::new();
        use std::io::Read as _;
        open_walked(&dir.join("real").join("key")).unwrap().read_to_string(&mut read).unwrap();
        assert_eq!(read, "secret");
        // A junction to the folder: refused where it's on the way.
        let junction = dir.join("junction");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(dir.join("real")).output().unwrap();
        assert!(made.status.success(), "{made:?}");
        assert!(std::fs::read(junction.join("key")).is_ok(), "it does lead there");
        let error = open_walked(&junction.join("key")).unwrap_err();
        assert!(error.to_string().contains("is a link"), "{error}");
        // A symbolic link to the file (Developer Mode lets this user make one).
        let link = dir.join("link");
        std::os::windows::fs::symlink_file(dir.join("real").join("key"), &link).unwrap();
        assert!(open_walked(&link).unwrap_err().to_string().contains("is a link"));
        // A folder, a relative path and a share's are refused too.
        assert!(open_walked(&dir.join("real")).is_err());
        assert!(open_walked(Path::new("real\\key")).is_err());
        assert!(open_walked(Path::new(r"\\server\share\key")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_follow_opens_and_locks_refuse_links_and_lock_once() {
        use factory_agent::windows::system::is_private_path;
        let dir = std::env::temp_dir().join(format!("claktory-nofollow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (real, link) = (dir.join("real"), dir.join("link"));
        std::fs::write(&real, b"kept").unwrap();
        std::os::windows::fs::symlink_file(&real, &link).unwrap();
        let mut write = std::fs::OpenOptions::new();
        write.write(true).create(true).truncate(false);
        assert!(is_link(&open_no_follow(&mut write.clone(), &link).unwrap_err()), "a link at the end is refused");
        assert_eq!(std::fs::read(&real).unwrap(), b"kept");
        open_no_follow(&mut write, &dir.join("new")).expect("a new file is made");
        open_no_follow(&mut write, &real).expect("a plain file opens");
        // A junction: refused as a lock file, and the folder it leads to untouched.
        let junction = dir.join("junction");
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&dir).output().unwrap();
        assert!(made.status.success(), "{made:?}");
        assert!(open_lock(&junction).is_err(), "a junction to a folder is no lock file");
        assert!(is_link(&open_lock(&link).unwrap_err()));
        // The lock: made private, held by one at a time, never deleted while open.
        let path = dir.join("lock");
        let held = open_lock(&path).unwrap();
        assert!(is_private_path(&path).unwrap());
        held.try_lock().expect("the first takes it");
        let other = open_lock(&path).unwrap();
        assert!(other.try_lock().is_err(), "a second waits its turn");
        assert!(std::fs::remove_file(&path).is_err(), "no one deletes it while it's open");
        drop(held);
        other.try_lock().expect("let go when the first closed");
        drop(other);
        // An existing lock file someone left open to others is made private.
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"").unwrap();
        drop(open_lock(&path).unwrap());
        assert!(is_private_path(&path).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_held_folder_is_neither_renamed_nor_moved() {
        let dir = std::env::temp_dir().join(format!("claktory-held-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("up").join("folder")).unwrap();
        let held = open_folder(&dir.join("up"), false).unwrap();
        assert!(std::fs::rename(dir.join("up"), dir.join("moved")).is_err(), "held, it isn't renamed");
        let inner = open_folder(&dir.join("up").join("folder"), false).unwrap();
        drop(held);
        // Held only below: Windows refuses to rename a folder with something open inside it.
        assert!(std::fs::rename(dir.join("up"), dir.join("moved")).is_err(), "nor one above a held folder");
        drop(inner);
        std::fs::rename(dir.join("up"), dir.join("moved")).expect("free again");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_change_time_moves_when_metadata_does() {
        let dir = std::env::temp_dir().join(format!("claktory-ctime-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("file");
        std::fs::write(&file, b"one").unwrap();
        let before = changed_ns(&file).unwrap();
        assert!(before > 1_700_000_000 * 1_000_000_000);
        std::thread::sleep(std::time::Duration::from_millis(20));
        let mut permissions = std::fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&file, permissions.clone()).unwrap();
        assert!(changed_ns(&file).unwrap() > before, "made read-only, its change time moved");
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&file, permissions).unwrap();
        assert!(!on_network_disk(&file).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_running_program_is_replaced_and_swept_once_it_has_stopped() {
        let dir = std::env::temp_dir().join(format!("claktory-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A copy of Windows' own `waitfor.exe`, running from where it's replaced.
        let program = dir.join("waiting.exe");
        let system = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap();
        std::fs::copy(system.join("System32").join("waitfor.exe"), &program).unwrap();
        let signal = format!("claktory{}", std::process::id());
        let mut running =
            std::process::Command::new(&program).args(["/T", "30", &signal]).stdout(std::process::Stdio::null()).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(std::fs::write(&program, b"new").is_err(), "a running program's file can't be written over");
        let new = dir.join("waiting.exe.tmp");
        std::fs::write(&new, b"new").unwrap();
        let aside = replace(&new, &program).unwrap().expect("the old one moved aside");
        assert_eq!(std::fs::read(&program).unwrap(), b"new");
        assert!(!new.exists());
        sweep_replaced(&program);
        assert!(aside.exists(), "still running");
        running.kill().unwrap();
        running.wait().unwrap();
        sweep_replaced(&program);
        assert!(!aside.exists(), "swept once it stopped");
        std::fs::write(&new, b"newer").unwrap();
        assert_eq!(replace(&new, &dir.join("fresh.exe")).unwrap(), None, "nothing to move aside");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_space_of_the_temporary_folders_volume() {
        let space = space(&std::env::temp_dir()).unwrap();
        assert!(space.total > 0 && space.free <= space.total && space.available <= space.free, "{space:?}");
        let root = volume_root(&std::env::temp_dir()).unwrap();
        assert!(root.to_string_lossy().ends_with(":\\"), "{}", root.display());
    }
}
