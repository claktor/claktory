//! Private files and folders on Windows: what mode 0600 and 0700 are on a
//! Mac or Linux. Each gets a protected DACL (it inherits nothing from the
//! folder it's in) giving full access to this user and SYSTEM only, set as
//! it's created, never after, so there's no moment it's readable by others.
//!
//! As 0600 is elsewhere, it keeps other users of the computer out, not this
//! user's own programs (agents' commands among them, under authority B).

use std::fs::File;
use std::io::Write;
use std::os::windows::io::OwnedHandle;
use std::path::Path;

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GENERIC_READ, GENERIC_WRITE};
use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
use windows_sys::Win32::Security::{ACL, DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION};
use windows_sys::Win32::Storage::FileSystem::{CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ};

use super::system::{Protected, hex, is_private_path, owned_by_user, random_bytes};
use super::wide::{check, owned, wide};

/// Makes `path` a private folder: created so if it's missing (its parents
/// as ordinary folders), made so if it's there (`protect`). What's made
/// inside it later inherits its DACL.
pub fn create_dir_all(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let protected = Protected::for_folder()?;
    let attributes = protected.attributes();
    let name = wide(path)?;
    // SAFETY: a NUL-terminated path and attributes that outlive the call.
    if unsafe { CreateDirectoryW(name.as_ptr(), &raw const attributes) } != 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_ALREADY_EXISTS as i32) && path.is_dir() {
        return protect(path);
    }
    Err(error)
}

/// Creates the private folder `path`, whose parent is there; fails if it's
/// there already, as `DirBuilder::create` does.
pub fn create_dir(path: &Path) -> std::io::Result<()> {
    let protected = Protected::for_folder()?;
    let attributes = protected.attributes();
    let name = wide(path)?;
    // SAFETY: a NUL-terminated path and attributes that outlive the call.
    check(unsafe { CreateDirectoryW(name.as_ptr(), &raw const attributes) })
}

/// Gives a file or folder that's already there the private DACL, in place
/// of whatever it had and inherited; a folder's is inherited by what's in
/// it. Nothing to do when it's private already (so a big home isn't walked
/// at every start). Refused for a junction or symbolic link, which would
/// make private whatever it points at, and for something another account
/// owns (Fugo 45257, LOW 3).
pub fn protect(path: &Path) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is a link to somewhere else, so Claktory won't make it private", path.display()),
        ));
    }
    if !owned_by_user(path)? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} belongs to another account, so Claktory won't use it", path.display()),
        ));
    }
    if is_private_path(path)? {
        return Ok(());
    }
    let protected = if metadata.is_dir() { Protected::for_folder()? } else { Protected::for_user()? };
    let dacl = protected.dacl()?;
    let name = wide(path)?;
    // SAFETY: a NUL-terminated path; the DACL lives in `protected` for the call.
    let result = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            dacl,
            std::ptr::null(),
        )
    };
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}

/// Creates a new private file, open to read and write; fails if it's
/// there already (as `create_new` does), so it never takes over another's.
pub fn create_new(path: &Path) -> std::io::Result<File> {
    let protected = Protected::for_user()?;
    let attributes = protected.attributes();
    let name = wide(path)?;
    // SAFETY: a NUL-terminated path; the handle is checked and owned.
    let handle: OwnedHandle = owned(unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ,
            &raw const attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    })?;
    Ok(File::from(handle))
}

/// Writes `bytes` to the private file `path`, replacing it whole: into a
/// new private file beside it, flushed, then renamed over it, so a reader
/// sees the old contents or the new, and the file is never anyone else's.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let file_name = path.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "a file needs a name"))?;
    let mut temporary_name = file_name.to_os_string();
    temporary_name.push(format!(".{}.new", hex(&random_bytes::<6>()?)));
    let temporary = path.with_file_name(temporary_name);
    let written = (|| {
        let mut file = create_new(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    if let Err(error) = written.and_then(|()| std::fs::rename(&temporary, path)) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

impl Protected {
    /// Its DACL, for SetNamedSecurityInfoW; lives as long as it does.
    fn dacl(&self) -> std::io::Result<*const ACL> {
        let (mut present, mut defaulted) = (0, 0);
        let mut dacl: *mut ACL = std::ptr::null_mut();
        // SAFETY: a valid descriptor; writes the three outputs.
        check(unsafe {
            GetSecurityDescriptorDacl(self.attributes().lpSecurityDescriptor, &raw mut present, &raw mut dacl, &raw mut defaulted)
        })?;
        if present == 0 || dacl.is_null() {
            return Err(std::io::Error::other("the private security descriptor has no DACL"));
        }
        Ok(dacl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("claktory-private-{}", hex(&random_bytes::<6>().unwrap())))
    }

    #[test]
    fn a_private_folder_and_file_are_made_and_replaced_whole() {
        let folder = scratch().join("inner");
        create_dir_all(&folder).expect("a private folder");
        create_dir_all(&folder).expect("made private again");
        assert_eq!(create_dir(&folder).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        create_dir(&folder.join("secrets")).expect("a new private folder");
        std::fs::remove_dir(folder.join("secrets")).unwrap();
        assert!(is_private_path(&folder).unwrap());
        let file = folder.join("app-token");
        write(&file, b"one").unwrap();
        write(&file, b"two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "no temporary file is left");
        assert_eq!(create_new(&file).unwrap_err().kind(), std::io::ErrorKind::AlreadyExists);
        assert!(is_private_path(&file).unwrap());
        // A file a library makes inside, with no DACL of its own, inherits the folder's.
        std::fs::write(folder.join("plain"), b"x").unwrap();
        assert!(!super::super::system::open_to_others(&folder.join("plain")).unwrap());
        std::fs::remove_file(folder.join("plain")).unwrap();
        std::fs::remove_dir_all(folder.parent().unwrap()).unwrap();
    }
}
