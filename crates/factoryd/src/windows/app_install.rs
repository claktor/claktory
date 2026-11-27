//! Puts a release's app in on Windows, for the launcher to start.
//!
//! The app is installed per user in `%LOCALAPPDATA%\Programs\Claktory`
//! (`root`): a small launcher, `Claktory.exe`, and each version in
//! `app\<version>\`. A running app's files can't be replaced, so a new
//! version goes in a folder of its own beside it, and the launcher starts
//! whichever `app\current` names (one line, the version). `app\previous`
//! names the one before, which the launcher goes back to if the new one
//! fails to start. Only the launcher deletes versions: those neither file
//! names, after a good start, and never while `app\installing` exists,
//! which this holds open (deleted when it's closed, a crash included) from
//! before the copy until `current` names the new version.

use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The app's own program in a version's folder.
const APP: &str = "Claktory.exe";
/// The launcher's place in a release's `app`.
const LAUNCHER: &str = "launcher";

/// The version a release's app goes in as: `v0.1.27` is `0.1.27`. Only
/// digits and single dots, up to 32 characters, as the launcher accepts.
pub fn app_version(release: &str) -> Option<String> {
    let version = release.strip_prefix('v').unwrap_or(release);
    let plain = !version.is_empty()
        && version.len() <= 32
        && version.split('.').all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    plain.then(|| version.to_owned())
}

/// The version a pointer file names, if it names one.
fn pointed(pointer: &Path) -> Option<String> {
    let text = std::fs::read_to_string(pointer).ok()?;
    app_version(text.trim()).filter(|version| !version.contains('v') && text.trim() == version)
}

/// Writes a pointer: plain ASCII in a temporary file, moved over the old
/// one and on the disk before this returns.
fn point(pointer: &Path, version: &str) -> std::io::Result<()> {
    let staged = pointer.with_extension("new");
    std::fs::write(&staged, format!("{version}\n"))?;
    super::fs::move_through(&staged, pointer)
}

/// Held open while a version goes in, so the launcher deletes nothing
/// meanwhile; deleted when it's closed.
fn hold_installing(app: &Path) -> std::io::Result<std::fs::File> {
    use windows_sys::Win32::Storage::FileSystem::{DELETE, FILE_FLAG_DELETE_ON_CLOSE, FILE_GENERIC_WRITE};
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .access_mode(FILE_GENERIC_WRITE | DELETE)
        .share_mode(0)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .open(app.join("installing"))
}

/// Copies `from` into `to`, leaving out the top folder's `launcher`; a link
/// anywhere fails it.
fn copy_version(from: &Path, to: &Path, top: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if top && entry.file_name().eq_ignore_ascii_case(LAUNCHER) {
            continue;
        }
        if kind.is_dir() {
            copy_version(&entry.path(), &to.join(entry.file_name()), false)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        } else {
            return Err(std::io::Error::other(format!("{} is a link", entry.path().display())));
        }
    }
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Puts `release`'s app in under `root` as `release_version`, checking its
/// programs with `check` (`authenticode::check_release` and
/// `check_program`), then points `app\current` at it. The launcher goes in
/// too when the release's differs, the old one moved aside to
/// `Claktory.exe.old`, which the launcher deletes at its next start.
pub fn install(
    root: &Path,
    release: &Path,
    release_version: &str,
    check_folder: impl Fn(&Path) -> Result<(), String>,
    check_program: impl Fn(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let failed = |what: &str, error: std::io::Error| format!("{what}: {error}");
    let Some(version) = app_version(release_version) else {
        return Err(format!("{release_version} isn't a version the app can be installed as"));
    };
    let source = release.join("app");
    if !source.join(APP).is_file() {
        return Err("the release has no app in it".to_owned());
    }
    let app = root.join("app");
    std::fs::create_dir_all(&app).map_err(|error| failed("can't make the app's folder", error))?;
    let installing = hold_installing(&app).map_err(|error| failed("another install is under way", error))?;
    let target = app.join(&version);
    let current = app.join("current");
    // One already there that isn't the version running (a copy the launcher
    // went back from, an earlier install that stopped, or anything else that
    // put it there) is copied again from the release, so a damaged copy
    // doesn't fail again. The running one is kept while it passes its check.
    let running = pointed(&current).as_deref() == Some(version.as_str());
    if target.exists() && !(running && target.join(APP).is_file() && check_folder(&target).is_ok()) {
        std::fs::remove_dir_all(&target).map_err(|error| failed("can't clear what was in the new version's place", error))?;
    }
    if !target.join(APP).is_file() {
        let partial = app.join(format!("{version}.partial"));
        let _ = std::fs::remove_dir_all(&partial);
        copy_version(&source, &partial, true).map_err(|error| failed("can't copy the app", error))?;
        if let Err(why) = check_folder(&partial) {
            let _ = std::fs::remove_dir_all(&partial);
            return Err(why);
        }
        let _ = std::fs::remove_dir_all(&target);
        std::fs::rename(&partial, &target).map_err(|error| failed("can't put the app in", error))?;
    }
    put_launcher(root, &source.join(LAUNCHER).join(APP), &check_program)?;
    match pointed(&current) {
        Some(before) if before == version => {}
        before => {
            if let Some(before) = before {
                point(&app.join("previous"), &before).map_err(|error| failed("can't write app\\previous", error))?;
            }
            point(&current, &version).map_err(|error| failed("can't write app\\current", error))?;
        }
    }
    drop(installing);
    Ok(())
}

/// The release's launcher at `root\Claktory.exe`, when it differs from the
/// one there: staged, checked, then put in with the old one moved aside.
fn put_launcher(root: &Path, new: &Path, check: &impl Fn(&Path) -> Result<(), String>) -> Result<(), String> {
    let installed = root.join(APP);
    if !new.is_file() || same_file(new, &installed) {
        return Ok(());
    }
    let staged: PathBuf = root.join(format!("{APP}.new"));
    std::fs::copy(new, &staged).map_err(|error| format!("can't copy the launcher: {error}"))?;
    if let Err(why) =
        check(&staged).and_then(|()| if same_file(&staged, new) { Ok(()) } else { Err("the launcher's copy isn't the same".to_owned()) })
    {
        let _ = std::fs::remove_file(&staged);
        return Err(why);
    }
    let old = root.join(format!("{APP}.old"));
    if installed.exists() {
        super::fs::move_through(&installed, &old).map_err(|error| format!("can't move the launcher aside: {error}"))?;
    }
    if let Err(error) = super::fs::move_through(&staged, &installed) {
        let _ = super::fs::move_through(&old, &installed);
        return Err(format!("can't put the launcher in: {error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(dir: &Path, app: &[u8], launcher: &[u8]) -> PathBuf {
        let release = dir.join(format!("release-{}", String::from_utf8_lossy(app)));
        std::fs::create_dir_all(release.join("app").join("data")).expect("release");
        std::fs::create_dir_all(release.join("app").join(LAUNCHER)).expect("launcher");
        std::fs::write(release.join("app").join(APP), app).expect("app");
        std::fs::write(release.join("app").join("data").join("app.so"), b"snapshot").expect("data");
        std::fs::write(release.join("app").join(LAUNCHER).join(APP), launcher).expect("launcher");
        release
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn versions_are_digits_and_dots_only() {
        assert_eq!(app_version("v0.1.27").as_deref(), Some("0.1.27"));
        assert_eq!(app_version("0.2.0").as_deref(), Some("0.2.0"));
        for bad in ["", "v", "2026.11.19-1958", "0..1", ".1", "1.", "0.1.x", "..\\x", &"1.".repeat(20)] {
            assert_eq!(app_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_version_goes_in_beside_the_one_running_and_the_pointers_follow() {
        let dir = std::env::temp_dir().join(format!("claktory-app-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("Claktory");
        let ok = |_: &Path| Ok(());
        install(&root, &release(&dir, b"one", b"launcher 1"), "v0.1.27", ok, ok).expect("first");
        let app = root.join("app");
        assert_eq!(read(&app.join("current")), "0.1.27\n");
        assert!(!app.join("previous").exists());
        assert_eq!(read(&app.join("0.1.27").join(APP)), "one");
        assert!(app.join("0.1.27/data/app.so").is_file());
        assert!(!app.join("0.1.27").join(LAUNCHER).exists(), "the launcher isn't part of a version");
        assert_eq!(read(&root.join(APP)), "launcher 1");
        assert!(!app.join("installing").exists(), "deleted once closed");

        install(&root, &release(&dir, b"two", b"launcher 2"), "v0.1.28", ok, ok).expect("second");
        assert_eq!((read(&app.join("current")), read(&app.join("previous"))), ("0.1.28\n".into(), "0.1.27\n".into()));
        assert!(app.join("0.1.27").is_dir(), "never deleted here");
        assert_eq!(read(&root.join(APP)), "launcher 2");
        assert_eq!(read(&root.join(format!("{APP}.old"))), "launcher 1");

        // The same version again changes nothing.
        install(&root, &release(&dir, b"two", b"launcher 2"), "v0.1.28", ok, ok).expect("again");
        assert_eq!(read(&app.join("previous")), "0.1.27\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_app_that_isnt_claktorys_own_never_goes_in_and_nothing_points_at_it() {
        let dir = std::env::temp_dir().join(format!("claktory-app-refused-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("Claktory");
        let ok = |_: &Path| Ok(());
        install(&root, &release(&dir, b"one", b"launcher 1"), "v0.1.27", ok, ok).expect("first");
        let refused = install(&root, &release(&dir, b"two", b"launcher 2"), "v0.1.28", |_| Err("unsigned".to_owned()), ok);
        assert_eq!(refused, Err("unsigned".to_owned()));
        let app = root.join("app");
        assert!(!app.join("0.1.28").exists() && !app.join("0.1.28.partial").exists());
        assert_eq!(read(&app.join("current")), "0.1.27\n");
        let refused = install(&root, &release(&dir, b"three", b"launcher 3"), "v0.1.29", ok, |_| Err("unsigned".to_owned()));
        assert_eq!(refused, Err("unsigned".to_owned()));
        assert_eq!(read(&root.join(APP)), "launcher 1");
        assert_eq!(read(&app.join("current")), "0.1.27\n", "the launcher failed first");
        assert!(!app.join("installing").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_already_in_place_that_fails_its_check_is_copied_again() {
        let dir = std::env::temp_dir().join(format!("claktory-app-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("Claktory");
        let planted = root.join("app").join("0.1.28");
        std::fs::create_dir_all(&planted).expect("planted");
        std::fs::write(planted.join(APP), "planted").expect("planted app");
        let ours = |folder: &Path| if read(&folder.join(APP)) == "two" { Ok(()) } else { Err("not ours".to_owned()) };
        install(&root, &release(&dir, b"two", b"launcher 2"), "v0.1.28", ours, |_| Ok(())).expect("installed");
        assert_eq!(read(&planted.join(APP)), "two");
        assert_eq!(read(&root.join("app").join("current")), "0.1.28\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_version_the_launcher_went_back_from_is_copied_again_from_the_release() {
        let dir = std::env::temp_dir().join(format!("claktory-app-again-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("Claktory");
        let ok = |_: &Path| Ok(());
        install(&root, &release(&dir, b"one", b"launcher 1"), "v0.1.27", ok, ok).expect("first");
        let newer = release(&dir, b"two", b"launcher 2");
        install(&root, &newer, "v0.1.28", ok, ok).expect("second");
        let app = root.join("app");
        // Its copy lost a file, it failed to start, and the launcher went back.
        std::fs::remove_file(app.join("0.1.28/data/app.so")).expect("damaged");
        std::fs::write(app.join("current"), "0.1.27\n").expect("went back");
        install(&root, &newer, "v0.1.28", ok, ok).expect("again");
        assert_eq!(read(&app.join("0.1.28/data/app.so")), "snapshot", "copied again");
        assert_eq!((read(&app.join("current")), read(&app.join("previous"))), ("0.1.28\n".into(), "0.1.27\n".into()));
        assert!(!app.join("0.1.28.partial").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_second_install_waits_its_turn() {
        let dir = std::env::temp_dir().join(format!("claktory-app-busy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("Claktory");
        std::fs::create_dir_all(root.join("app")).expect("app");
        let held = hold_installing(&root.join("app")).expect("held");
        let ok = |_: &Path| Ok(());
        let busy = install(&root, &release(&dir, b"one", b"launcher 1"), "v0.1.27", ok, ok).unwrap_err();
        assert!(busy.contains("another install"), "{busy}");
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
