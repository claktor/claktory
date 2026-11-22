//! `--register` and `--unregister`: the one file that lets Chrome start this
//! helper, its native messaging manifest in Google Chrome's user-level
//! NativeMessagingHosts. Only that file, written whole (a temporary file,
//! then a rename), naming exactly this helper and exactly one extension.
//! Another program's file of the same name is left alone unless the person
//! chose to replace it; a link, file or folder, is never followed. The app
//! runs these only after the person agreed in its setup.

use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// What to register.
pub struct Manifest<'a> {
    pub host: &'a str,
    /// The extension's origin, `chrome-extension://<id>/`.
    pub caller: &'a str,
    /// This helper, absolute.
    pub path: &'a Path,
}

impl Manifest<'_> {
    fn json(&self) -> Value {
        json!({
            "name": self.host,
            "description": "Claktory's browser helper",
            "path": self.path.to_string_lossy(),
            "type": "stdio",
            "allowed_origins": [self.caller],
        })
    }

    /// `existing` is this helper's manifest for this extension, wherever its
    /// helper was (an app moved or updated is repaired).
    fn owns(&self, existing: &Value) -> bool {
        existing.get("name").and_then(Value::as_str) == Some(self.host) && existing.get("allowed_origins") == Some(&json!([self.caller]))
    }
}

/// The helper in place for registering: absolute, and, in a real build,
/// inside an app's Contents/Helpers.
pub fn helper_path() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().and_then(|exe| exe.canonicalize()).map_err(|_| "this helper can't find itself".to_owned())?;
    if !cfg!(feature = "dev") && crate::flavor::bundle_of(&exe).is_none() {
        return Err(format!("{} isn't inside an app's Contents/Helpers", exe.display()));
    }
    Ok(exe)
}

fn read_existing(file: &Path) -> Result<Option<Value>, String> {
    match std::fs::symlink_metadata(file) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            Err(format!("{} isn't a plain file; it's left as it is", file.display()))
        }
        Ok(_) => Ok(Some(std::fs::read(file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or(Value::Null))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", file.display())),
    }
}

/// The manifests' folder, made if Chrome is there but it isn't yet; never a
/// link.
fn folder(file: &Path) -> Result<&Path, String> {
    let dir = file.parent().ok_or("the manifest has no folder")?;
    let chrome = dir.parent().ok_or("the manifest has no folder")?;
    let is_dir = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink());
    if !is_dir(chrome) {
        return Err(format!("{} isn't there: open Google Chrome once first", chrome.display()));
    }
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => Err(format!("{} isn't a folder", dir.display())),
        Ok(meta) if meta.uid() != unsafe { libc::getuid() } => Err(format!("{} belongs to another user", dir.display())),
        Ok(_) => Ok(dir),
        Err(_) => {
            std::fs::create_dir(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
            Ok(dir)
        }
    }
}

/// Writes the manifest at `file`. `replace`: the person chose to replace
/// another program's file of that name.
pub fn register(file: &Path, manifest: &Manifest<'_>, replace: bool) -> Result<(), String> {
    if !manifest.path.is_absolute() {
        return Err("the helper's path isn't absolute".into());
    }
    let dir = folder(file)?;
    if let Some(existing) = read_existing(file)?
        && !manifest.owns(&existing)
        && !replace
    {
        return Err(format!("{} belongs to another program; it's left as it is", file.display()));
    }
    let name = file.file_name().ok_or("the manifest has no name")?.to_string_lossy();
    let temporary = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut out =
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).custom_flags(libc::O_NOFOLLOW).open(&temporary)?;
        out.write_all(serde_json::to_string_pretty(&manifest.json())?.as_bytes())?;
        out.write_all(b"\n")?;
        out.sync_all()?;
        std::fs::rename(&temporary, file)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("{}: {error}", file.display()));
    }
    Ok(())
}

/// Removes the manifest at `file`, only when it's this helper's: its
/// extension, and its path this very helper.
pub fn unregister(file: &Path, manifest: &Manifest<'_>) -> Result<bool, String> {
    let Some(existing) = read_existing(file)? else { return Ok(false) };
    let ours = manifest.owns(&existing) && existing.get("path").and_then(Value::as_str) == Some(&*manifest.path.to_string_lossy());
    if !ours {
        return Err(format!("{} isn't this helper's; it's left as it is", file.display()));
    }
    std::fs::remove_file(file).map_err(|error| format!("{}: {error}", file.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_its_own_manifest_written_whole_and_removed_only_when_it_points_here() {
        let dir = tempfile::tempdir().unwrap();
        let chrome = dir.path().join("Google/Chrome");
        let file = chrome.join("NativeMessagingHosts/com.claktory.browser.dev.json");
        let helper = dir.path().join("Claktory Dev.app/Contents/Helpers/claktory-browser-host");
        let manifest = Manifest { host: "com.claktory.browser.dev", caller: "chrome-extension://abc/", path: &helper };
        assert!(register(&file, &manifest, false).unwrap_err().contains("open Google Chrome once"), "never makes Chrome's folder");

        std::fs::create_dir_all(&chrome).unwrap();
        register(&file, &manifest, false).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(written["allowed_origins"], json!(["chrome-extension://abc/"]));
        assert_eq!(written["path"], helper.to_string_lossy().as_ref());
        assert_eq!(written["type"], "stdio");
        assert_eq!(std::fs::read_dir(file.parent().unwrap()).unwrap().count(), 1, "no temporary file left");

        // Moved, the app repairs its own manifest.
        let moved = dir.path().join("Apps/Claktory Dev.app/Contents/Helpers/claktory-browser-host");
        let moved_manifest = Manifest { path: &moved, ..manifest };
        register(&file, &moved_manifest, false).unwrap();
        // Only the helper it points at removes it.
        let manifest = Manifest { host: "com.claktory.browser.dev", caller: "chrome-extension://abc/", path: &helper };
        assert!(unregister(&file, &manifest).is_err());
        assert!(unregister(&file, &moved_manifest).unwrap());
        assert!(!file.exists());
        assert!(!unregister(&file, &moved_manifest).unwrap(), "nothing to remove");

        // Another program's file of that name, or another extension's.
        std::fs::write(&file, r#"{"name": "com.claktory.browser.dev", "allowed_origins": ["chrome-extension://evil/"]}"#).unwrap();
        assert!(register(&file, &manifest, false).unwrap_err().contains("belongs to another program"));
        assert!(unregister(&file, &manifest).is_err());
        register(&file, &manifest, true).unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(written["allowed_origins"], json!(["chrome-extension://abc/"]));

        // A link is never followed, as the file or the folder.
        std::fs::remove_file(&file).unwrap();
        let elsewhere = dir.path().join("elsewhere.json");
        std::fs::write(&elsewhere, "{}").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
        assert!(register(&file, &manifest, true).unwrap_err().contains("isn't a plain file"));
        assert!(unregister(&file, &manifest).is_err());
        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "{}");
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(file.parent().unwrap()).unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, file.parent().unwrap()).unwrap();
        assert!(register(&file, &manifest, false).unwrap_err().contains("isn't a folder"));
        assert_eq!(std::fs::read_dir(&target).unwrap().count(), 0);
        assert!(!register(&file, &Manifest { path: Path::new("relative"), ..manifest }, false).is_ok());
    }
}
