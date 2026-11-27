//! The explorers' folders, watched for changes (Polpo's explorer.md): each
//! folder an explorer lists (its roots and their open folders) and each
//! root's `.factoryignore`, without what's below them, so a project's tree
//! is listed again when something in it changes instead of twice a second.
//!
//! kqueue on a Mac reports a watched folder's own change (an entry added,
//! removed or renamed). inotify on Linux reports its entries' changes, of
//! which only those that change a listing count: an entry made, removed or
//! renamed, never a file's contents but the `.factoryignore`'s. What
//! watches miss (a network volume, a root that didn't exist yet, past the
//! most watches) is seen at the explorers' backstop (`factory::explore`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use notify::event::{EventKind, ModifyKind};
use notify::{RecursiveMode, Watcher};
use tokio::sync::Notify;

use crate::explorer::IGNORE_FILE;

/// The most folders and files watched at once (one open file each on a
/// Mac); past it, the rest is seen at the backstop.
const MOST: usize = 2_000;

/// What one project's explorer read: its folders, and its roots'
/// `.factoryignore` files that are there.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub(crate) struct Listed {
    pub(crate) folders: BTreeSet<PathBuf>,
    pub(crate) files: BTreeSet<PathBuf>,
}

/// What the watcher's thread and the explorers share.
#[derive(Default)]
struct Shared {
    /// The projects whose explorers list each folder watched.
    folders: HashMap<PathBuf, BTreeSet<String>>,
    /// The projects whose roots each `.factoryignore` watched decides.
    files: HashMap<PathBuf, BTreeSet<String>>,
    /// The projects to list again.
    dirty: BTreeSet<String>,
    /// Every project is to be listed again: a watch failed.
    all: bool,
    /// Watched paths removed or renamed, whose watches ended with them.
    lost: HashSet<PathBuf>,
}

impl Shared {
    /// Notes what `event` changes; whether anything is to be listed again.
    fn note(&mut self, event: &notify::Event) -> bool {
        if matches!(event.kind, EventKind::Access(_)) {
            return false;
        }
        if event.paths.is_empty() {
            self.all = true;
            return true;
        }
        let gone = matches!(event.kind, EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_)));
        // A file's contents or metadata: no listing shows them.
        let contents = matches!(event.kind, EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Metadata(_)));
        let mut changed = false;
        for path in &event.paths {
            let projects = if let Some(projects) = self.folders.get(path).or_else(|| self.files.get(path)) {
                // The folder or file watched itself (kqueue says a folder's
                // entries changed this way).
                if gone {
                    self.lost.insert(path.clone());
                }
                projects
            } else if let Some(projects) = path.parent().and_then(|folder| self.folders.get(folder))
                && (!contents || path.file_name().is_some_and(|name| name == IGNORE_FILE))
            {
                // One of a watched folder's entries (inotify).
                projects
            } else {
                continue;
            };
            for project in projects {
                changed |= self.dirty.insert(project.clone());
            }
        }
        changed
    }
}

/// What's watched, and for which projects.
#[derive(Default)]
struct Watching {
    watcher: Option<notify::RecommendedWatcher>,
    by_project: HashMap<String, Listed>,
    folders: HashSet<PathBuf>,
    files: HashSet<PathBuf>,
    /// Said once that there are more than `MOST` to watch.
    full: bool,
}

pub(crate) struct ExplorerWatch {
    shared: Arc<Mutex<Shared>>,
    /// Notified when an explorer is to be listed again.
    pub(crate) changed: Arc<Notify>,
    watching: Mutex<Watching>,
}

impl Default for ExplorerWatch {
    fn default() -> ExplorerWatch {
        let shared: Arc<Mutex<Shared>> = Arc::default();
        let changed = Arc::new(Notify::new());
        let watcher = {
            let (shared, changed) = (shared.clone(), changed.clone());
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let mut shared = shared.lock().unwrap_or_else(PoisonError::into_inner);
                let noted = match event {
                    Ok(event) => shared.note(&event),
                    Err(_) => {
                        shared.all = true;
                        true
                    }
                };
                if noted {
                    changed.notify_one();
                }
            })
        };
        let watcher = watcher
            .inspect_err(|error| tracing::warn!(%error, "can't watch the explorers' folders; listing them at the backstop only"))
            .ok();
        ExplorerWatch { shared, changed, watching: Mutex::new(Watching { watcher, ..Watching::default() }) }
    }
}

impl ExplorerWatch {
    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The projects to list again since last asked, or None for all of them.
    pub(crate) fn take(&self) -> Option<BTreeSet<String>> {
        let mut shared = self.shared();
        let dirty = std::mem::take(&mut shared.dirty);
        (!std::mem::take(&mut shared.all)).then_some(dirty)
    }

    /// Watches what the explorers of `open` read: `listed` for the ones
    /// just listed, what they read before for the others. `again`: watches
    /// everything anew (the backstop), in case a watch ended unseen.
    pub(crate) fn set(&self, listed: Vec<(String, Listed)>, open: &BTreeSet<String>, again: bool) {
        let mut watching = self.watching.lock().unwrap_or_else(PoisonError::into_inner);
        let Watching { watcher, by_project, folders, files, full } = &mut *watching;
        by_project.retain(|project, _| open.contains(project));
        by_project.extend(listed);
        let (mut by_folder, mut by_file) = (HashMap::<PathBuf, BTreeSet<String>>::new(), HashMap::<PathBuf, BTreeSet<String>>::new());
        for (project, read) in by_project.iter() {
            for folder in &read.folders {
                by_folder.entry(folder.clone()).or_default().insert(project.clone());
            }
            for file in &read.files {
                by_file.entry(file.clone()).or_default().insert(project.clone());
            }
        }
        let lost = {
            let mut shared = self.shared();
            shared.folders.clone_from(&by_folder);
            shared.files.clone_from(&by_file);
            std::mem::take(&mut shared.lost)
        };
        let Some(watcher) = watcher.as_mut() else { return };
        let gone = |path: &PathBuf, wanted: &HashMap<PathBuf, BTreeSet<String>>| !wanted.contains_key(path) || again || lost.contains(path);
        for path in folders.extract_if(|path| gone(path, &by_folder)).chain(files.extract_if(|path| gone(path, &by_file))) {
            let _ = watcher.unwatch(&path);
        }
        let mut wanted: Vec<(PathBuf, bool)> = by_folder.into_keys().map(|path| (path, true)).collect();
        wanted.extend(by_file.into_keys().map(|path| (path, false)));
        // Folders first, roots before what's inside them, the same each time.
        wanted.sort_by(|(a, a_folder), (b, b_folder)| {
            b_folder.cmp(a_folder).then_with(|| a.components().count().cmp(&b.components().count())).then_with(|| a.cmp(b))
        });
        for (path, is_folder) in wanted {
            if if is_folder { folders.contains(&path) } else { files.contains(&path) } {
                continue;
            }
            if folders.len() + files.len() >= MOST {
                if !*full {
                    tracing::warn!("the explorers list more than {MOST} folders; the rest is listed at the backstop only");
                    *full = true;
                }
                break;
            }
            let watched = if is_folder { &mut *folders } else { &mut *files };
            if watcher.watch(&path, RecursiveMode::NonRecursive).is_ok() {
                watched.insert(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, DataChange, MetadataKind, RemoveKind, RenameMode};

    fn event(kind: EventKind, path: &str) -> notify::Event {
        notify::Event::new(kind).add_path(PathBuf::from(path))
    }

    fn shared() -> Shared {
        let mut shared = Shared::default();
        shared.folders.insert("/p/site".into(), BTreeSet::from(["site".to_owned()]));
        shared.folders.insert("/p/site/src".into(), BTreeSet::from(["site".to_owned()]));
        shared.folders.insert("/p/shared".into(), BTreeSet::from(["site".to_owned(), "blog".to_owned()]));
        shared.files.insert("/p/site/.factoryignore".into(), BTreeSet::from(["site".to_owned()]));
        shared
    }

    fn dirty(shared: &mut Shared) -> Vec<String> {
        std::mem::take(&mut shared.dirty).into_iter().collect()
    }

    /// kqueue (a Mac): a watched folder says its entries changed, as a
    /// change of its own; a removed or renamed watch is lost.
    #[test]
    fn a_watched_folders_own_change_counts() {
        let mut shared = shared();
        assert!(shared.note(&event(EventKind::Modify(ModifyKind::Data(DataChange::Any)), "/p/site/src")));
        assert_eq!(dirty(&mut shared), ["site"]);
        assert!(shared.note(&event(EventKind::Modify(ModifyKind::Any), "/p/shared")));
        assert_eq!(dirty(&mut shared), ["blog", "site"]);
        assert!(shared.note(&event(EventKind::Remove(RemoveKind::Any), "/p/site/src")));
        assert!(shared.lost.contains(&PathBuf::from("/p/site/src")));
        // The .factoryignore written in place.
        shared.dirty.clear();
        assert!(shared.note(&event(EventKind::Modify(ModifyKind::Data(DataChange::Any)), "/p/site/.factoryignore")));
        assert_eq!(dirty(&mut shared), ["site"]);
        // Somewhere not watched: neither a watched folder nor one of its
        // entries.
        assert!(!shared.note(&event(EventKind::Modify(ModifyKind::Any), "/p/elsewhere")));
        assert!(!shared.note(&event(EventKind::Create(CreateKind::File), "/p/site/target/debug/x")));
    }

    /// inotify (Linux): a watched folder's entries say what changed; an
    /// entry made, removed or renamed counts, a file's contents don't, but
    /// the .factoryignore's do.
    #[test]
    fn a_watched_folders_entries_count_only_when_the_listing_changes() {
        let mut shared = shared();
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Create(CreateKind::Folder),
            EventKind::Remove(RemoveKind::File),
            EventKind::Modify(ModifyKind::Name(RenameMode::From)),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
        ] {
            assert!(shared.note(&event(kind, "/p/site/src/lib.rs")), "{kind:?}");
            assert_eq!(dirty(&mut shared), ["site"], "{kind:?}");
        }
        for kind in [
            EventKind::Modify(ModifyKind::Data(DataChange::Content)),
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::WriteTime)),
            EventKind::Access(notify::event::AccessKind::Close(notify::event::AccessMode::Write)),
        ] {
            assert!(!shared.note(&event(kind, "/p/site/src/lib.rs")), "{kind:?}");
        }
        assert!(shared.note(&event(EventKind::Modify(ModifyKind::Data(DataChange::Content)), "/p/shared/.factoryignore")));
        assert_eq!(dirty(&mut shared), ["blog", "site"]);
        // A rename's both ends, one of them watched.
        let both = notify::Event::new(EventKind::Modify(ModifyKind::Name(RenameMode::Both)))
            .add_path("/tmp/x".into())
            .add_path("/p/site/README.md".into());
        assert!(shared.note(&both));
        assert_eq!(dirty(&mut shared), ["site"]);
    }

    #[test]
    fn an_event_without_paths_lists_everything_again() {
        let mut shared = shared();
        assert!(shared.note(&notify::Event::new(EventKind::Other)));
        assert!(shared.all);
    }
}
