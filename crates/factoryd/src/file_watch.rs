//! Files watched for changes (`factory::watch`): the settings, the sign-ins
//! and the latest release, read again when the system says one changed
//! instead of every half second (#454). Each file is watched, and its folder
//! (or its nearest folder that exists, while it doesn't), so a file replaced
//! by a rename, or made later, is seen too.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use notify::{RecursiveMode, Watcher};
use tokio::sync::Notify;

pub(crate) struct FileWatch {
    watcher: Option<notify::RecommendedWatcher>,
    files: Vec<PathBuf>,
    /// The names an event has to touch to count: the files', their folders'
    /// and the missing folders' on the way to them.
    names: Arc<Mutex<HashSet<OsString>>>,
    folders: HashSet<PathBuf>,
    /// Notified when one of the files (or a folder on the way) may have
    /// changed, or the watch failed.
    pub(crate) changed: Arc<Notify>,
}

impl FileWatch {
    pub(crate) fn new(files: Vec<PathBuf>) -> FileWatch {
        let changed = Arc::new(Notify::new());
        let names: Arc<Mutex<HashSet<OsString>>> = Arc::default();
        let watcher = {
            let changed = changed.clone();
            let names = names.clone();
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else {
                    // A watch that failed: look anyway.
                    changed.notify_one();
                    return;
                };
                let names = names.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if event.paths.is_empty()
                    || event.paths.iter().any(|path| path.file_name().is_some_and(|name| names.contains(name)) && !written_inside(&event, path))
                {
                    changed.notify_one();
                }
            })
        };
        let watcher = match watcher {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                tracing::warn!(%error, "can't watch the settings for changes; reading them every half second");
                None
            }
        };
        let mut watch = FileWatch { watcher, files, names, folders: HashSet::new(), changed };
        watch.ensure();
        watch
    }

    /// Watches each file again (one replaced by a rename is another file)
    /// and its folder, or the nearest one that exists. Whether everything is
    /// watched: when not, the files are read every half second meanwhile.
    pub(crate) fn ensure(&mut self) -> bool {
        let Some(watcher) = self.watcher.as_mut() else { return false };
        let mut names = HashSet::new();
        let mut all = true;
        for file in &self.files {
            names.extend(file.file_name().map(OsString::from));
            let _ = watcher.unwatch(file);
            if file.exists() && watcher.watch(file, RecursiveMode::NonRecursive).is_err() {
                all = false;
            }
            let mut folder = file.parent();
            while let Some(missing) = folder.filter(|folder| !folder.exists()) {
                names.extend(missing.file_name().map(OsString::from));
                folder = missing.parent();
            }
            let Some(folder) = folder else {
                all = false;
                continue;
            };
            names.extend(folder.file_name().map(OsString::from));
            if !self.folders.contains(folder) {
                if watcher.watch(folder, RecursiveMode::NonRecursive).is_ok() {
                    self.folders.insert(folder.to_owned());
                } else {
                    all = false;
                }
            }
        }
        *self.names.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = names;
        all
    }
}

/// On Windows a folder is reported changed whenever anything in it is
/// written, files no one watches too: only its being made, renamed or
/// removed counts there.
#[cfg(windows)]
fn written_inside(event: &notify::Event, path: &std::path::Path) -> bool {
    use notify::EventKind::Modify;
    use notify::event::ModifyKind;
    matches!(event.kind, Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Metadata(_) | ModifyKind::Other)) && path.is_dir()
}

#[cfg(not(windows))]
fn written_inside(_event: &notify::Event, _path: &std::path::Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn told(watch: &mut FileWatch) -> bool {
        let told = tokio::time::timeout(Duration::from_secs(5), watch.changed.notified()).await.is_ok();
        watch.ensure();
        told
    }

    #[tokio::test]
    async fn a_file_made_later_written_or_replaced_is_told_of() {
        let root = std::env::temp_dir().join(format!("factoryd-file-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("later").join("config.json");
        let mut watch = FileWatch::new(vec![file.clone()]);
        assert!(watch.ensure());

        // Its folder, then the file, made later.
        std::fs::create_dir(root.join("later")).unwrap();
        assert!(told(&mut watch).await, "its folder made");
        std::fs::write(&file, "{}").unwrap();
        assert!(told(&mut watch).await, "the file made");
        // Written in place.
        std::fs::write(&file, "{\"a\": 1}").unwrap();
        assert!(told(&mut watch).await, "written in place");
        // Replaced by a rename, twice (the second on the new file).
        for round in 0..2 {
            let other = root.join("later").join(format!("config.json.new{round}"));
            std::fs::write(&other, format!("{{\"round\": {round}}}")).unwrap();
            std::fs::rename(&other, &file).unwrap();
            assert!(told(&mut watch).await, "replaced, round {round}");
            while tokio::time::timeout(Duration::from_millis(200), watch.changed.notified()).await.is_ok() {}
            watch.ensure();
        }
        std::fs::write(&file, "{\"b\": 2}").unwrap();
        assert!(told(&mut watch).await, "written in place after the renames");

        // Writes to other files there don't count.
        let other = root.join("later").join("factory.db-wal");
        std::fs::write(&other, "x").unwrap();
        while tokio::time::timeout(Duration::from_millis(300), watch.changed.notified()).await.is_ok() {}
        watch.ensure();
        std::fs::write(&other, "xy").unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(500), watch.changed.notified()).await.is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
