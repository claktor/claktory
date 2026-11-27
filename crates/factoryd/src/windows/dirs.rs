//! Where Claktory lives on Windows (windows.md §1): one resolver the
//! backend, its setup and the app's hooks agree on, from Windows' Known
//! Folders, never a guessed `C:\Users\<name>` or `%HOME%`.
//!
//! ```text
//! %LOCALAPPDATA%\Programs\Claktory\   the app, the backend, the helpers, the shell
//! %LOCALAPPDATA%\Claktory\factory\    the real Claktory's home (config, factory.db, vault, agents…)
//! %LOCALAPPDATA%\Claktory\factory-dev\ the dev Claktory's: never the same home
//! %LOCALAPPDATA%\Claktory\app\        the app's own few things on this device (app-dev for Dev)
//! ```
//!
//! Local, never Roaming, AppData: nothing here follows the person to another
//! computer, and no executable sits in a synced folder. `FACTORY_HOME`
//! still overrides the home (the backend's callers apply it, as on a Mac),
//! for development and QA.

use std::path::PathBuf;

use windows_sys::Win32::System::Com::CoTaskMemFree;
use windows_sys::Win32::UI::Shell::{FOLDERID_LocalAppData, FOLDERID_Profile, KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath};
use windows_sys::core::GUID;

/// The folder inside LocalAppData that's Claktory's.
const PRODUCT: &str = "Claktory";

fn known_folder(id: &GUID) -> std::io::Result<PathBuf> {
    let mut path = std::ptr::null_mut();
    // SAFETY: a known folder's id; the path Windows allocates is freed below.
    let result = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY as u32, std::ptr::null_mut(), &raw mut path) };
    if result < 0 {
        // SAFETY: freeing null is allowed, and Windows may have allocated even on failure.
        unsafe { CoTaskMemFree(path.cast()) };
        return Err(std::io::Error::from_raw_os_error(result));
    }
    let mut length = 0;
    // SAFETY: a NUL-terminated string Windows returned.
    while unsafe { *path.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: `length` units were just read.
    let units = unsafe { std::slice::from_raw_parts(path, length) };
    let folder = PathBuf::from(String::from_utf16_lossy(units));
    // SAFETY: allocated by SHGetKnownFolderPath.
    unsafe { CoTaskMemFree(path.cast()) };
    if !folder.is_absolute() {
        return Err(std::io::Error::other(format!("Windows gave a known folder that isn't absolute: {}", folder.display())));
    }
    Ok(folder)
}

/// This user's LocalAppData (`C:\Users\<name>\AppData\Local`, or wherever
/// they or their organization moved it).
pub fn local_app_data() -> std::io::Result<PathBuf> {
    known_folder(&FOLDERID_LocalAppData)
}

/// This user's profile folder: what agents' commands get as HOME.
pub fn profile() -> std::io::Result<PathBuf> {
    known_folder(&FOLDERID_Profile)
}

/// The home of the real Claktory (`dev` false) or the dev one.
pub fn factory_home(dev: bool) -> std::io::Result<PathBuf> {
    Ok(local_app_data()?.join(PRODUCT).join(if dev { "factory-dev" } else { "factory" }))
}

/// Where the app keeps its own few things on this device.
pub fn app_data(dev: bool) -> std::io::Result<PathBuf> {
    Ok(local_app_data()?.join(PRODUCT).join(if dev { "app-dev" } else { "app" }))
}

/// Where the installer puts Claktory's programs, per user.
pub fn programs() -> std::io::Result<PathBuf> {
    Ok(local_app_data()?.join("Programs").join(PRODUCT))
}

/// The backend's own Windows records inside a home: the commands' jobs, and
/// later the app's token. Private (`factory_agent::windows::private`).
pub fn windows_records(home: &std::path::Path) -> PathBuf {
    home.join("windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_homes_are_in_local_app_data_and_apart() {
        let local = local_app_data().expect("LocalAppData");
        assert!(local.is_absolute());
        let real = factory_home(false).unwrap();
        let dev = factory_home(true).unwrap();
        assert!(real.starts_with(&local) && dev.starts_with(&local));
        assert_ne!(real, dev);
        assert!(real.ends_with(r"Claktory\factory"));
        assert!(programs().unwrap().ends_with(r"Programs\Claktory"));
        assert_ne!(app_data(false).unwrap(), app_data(true).unwrap());
        assert!(profile().unwrap().is_absolute());
    }
}
