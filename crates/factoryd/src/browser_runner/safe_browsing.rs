//! Safe Browsing's lists for a new profile. Chrome fetches them only some
//! minutes after a user-data-dir's first start, and until then flags
//! nothing: exactly an agent's first browsing. So a profile without its
//! own lists gets a copy of the seed, lists a release's build fetched
//! (`SEED`, root's: in the Mac's image, or put there by setup on Linux),
//! before its Chrome first starts; Chrome brings them up to date within
//! minutes. A profile's own lists are never replaced.
//!
//! Whether the seed is root's alone is decided before the runner's user
//! namespace (`usable`, from `namespace`), where root's files show as
//! nobody's: by the gate on Linux, by the runner itself in the Mac's VM.
//! A seed that isn't is hidden there under an empty folder, so the runner,
//! which can't tell root's files from anyone's, finds none.
//!
//! While a profile has no lists all the same (no seed here), the runner
//! says so (`Report::Ready`'s `safe_browsing`), and says again once Chrome
//! has fetched them (`Report::SafeBrowsingReady`).

use std::path::{Path, PathBuf};

/// Where the seed is, the same in the Mac's image and on a Linux host.
pub(crate) const SEED: &str = "/usr/local/share/claktory-browser/safe-browsing";
/// The lists' folder in Chrome's user-data-dir.
const FOLDER: &str = "Safe Browsing";
/// The most a seed may hold, all its files together.
const MOST: u64 = 256 * 1024 * 1024;
/// How deep a seed's folders may go.
const DEPTH: usize = 4;

/// Copies `seed` into `user_data`'s Safe Browsing folder when it has none:
/// Ok(true) when copied, Ok(false) when the profile has its own or there's
/// no seed with lists. Its owners were checked before the namespace.
pub(crate) fn seed(user_data: &Path, seed: &Path) -> Result<bool, String> {
    let folder = user_data.join(FOLDER);
    if std::fs::symlink_metadata(&folder).is_ok() {
        return Ok(false);
    }
    match std::fs::symlink_metadata(seed) {
        Ok(meta) if meta.is_dir() && has_lists(seed) => {}
        _ => return Ok(false),
    }
    let staging = user_data.join(".safe-browsing-seeding");
    let _ = std::fs::remove_dir_all(&staging);
    let mut total = 0;
    let copied = copy(seed, &staging, 0, &mut total).and_then(|()| std::fs::rename(&staging, &folder).map_err(|error| format!("couldn't put the lists in place: {error}")));
    if copied.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    copied.map(|()| true)
}

/// Whether `seed` is a folder that only root could have written, all of
/// it: only meaningful outside the runner's user namespace.
pub(crate) fn usable(seed: &Path) -> bool {
    std::fs::symlink_metadata(seed).is_ok_and(|meta| meta.is_dir()) && seed_ok(seed).is_ok()
}

fn seed_ok(seed: &Path) -> Result<(), String> {
    super::gate::roots_alone(seed).map_err(|why| format!("Safe Browsing's seed isn't root's alone ({why}), so it isn't used"))?;
    for entry in walk(seed, 0)? {
        super::gate::roots_alone(&entry).map_err(|why| format!("Safe Browsing's seed isn't root's alone ({why}), so it isn't used"))?;
    }
    Ok(())
}

/// Every file and folder under `folder`, links and the rest left out.
fn walk(folder: &Path, depth: usize) -> Result<Vec<PathBuf>, String> {
    if depth > DEPTH {
        return Err("Safe Browsing's seed goes too deep".to_owned());
    }
    let mut found = Vec::new();
    let entries = std::fs::read_dir(folder).map_err(|error| format!("couldn't read Safe Browsing's seed: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("couldn't read Safe Browsing's seed: {error}"))?;
        let kind = entry.file_type().map_err(|error| format!("couldn't read Safe Browsing's seed: {error}"))?;
        if kind.is_dir() {
            found.extend(walk(&entry.path(), depth + 1)?);
            found.push(entry.path());
        } else if kind.is_file() {
            found.push(entry.path());
        }
    }
    Ok(found)
}

/// `from`'s folders and regular files into a new folder `to`, private to
/// the browser's user; links and anything else are left out.
fn copy(from: &Path, to: &Path, depth: usize, total: &mut u64) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    if depth > DEPTH {
        return Err("Safe Browsing's seed goes too deep".to_owned());
    }
    std::fs::DirBuilder::new().mode(0o700).create(to).map_err(|error| format!("couldn't copy Safe Browsing's lists: {error}"))?;
    let entries = std::fs::read_dir(from).map_err(|error| format!("couldn't read Safe Browsing's seed: {error}"))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("couldn't read Safe Browsing's seed: {error}"))?;
        let kind = entry.file_type().map_err(|error| format!("couldn't read Safe Browsing's seed: {error}"))?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy(&entry.path(), &target, depth + 1, total)?;
        } else if kind.is_file() {
            let mut source = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(entry.path())
                .map_err(|error| format!("couldn't read Safe Browsing's seed: {error}"))?;
            let mut copy = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&target)
                .map_err(|error| format!("couldn't copy Safe Browsing's lists: {error}"))?;
            let left = (MOST - *total) + 1;
            let copied = std::io::copy(&mut std::io::Read::take(&mut source, left), &mut copy).map_err(|error| format!("couldn't copy Safe Browsing's lists: {error}"))?;
            *total += copied;
            if *total > MOST {
                return Err("Safe Browsing's seed is too big".to_owned());
            }
        }
    }
    Ok(())
}

/// The lists a profile must have to count as having Safe Browsing's:
/// phishing and malware.
const NEEDED: [&str; 2] = ["UrlSoceng.store", "UrlMalware.store"];

/// Whether `user_data` has Safe Browsing's lists: each of `NEEDED` there,
/// with a data file beside it (`<name>.<digits>_<digits>`) that has
/// something in it. The small store file alone comes before any data.
pub(crate) fn ready(user_data: &Path) -> bool {
    has_lists(&user_data.join(FOLDER))
}

/// Whether the seed is here, root's alone, with the lists `ready` wants:
/// what setup installs on Linux.
pub(crate) fn seed_in_place(seed: &Path) -> bool {
    usable(seed) && has_lists(seed)
}

fn has_lists(folder: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(folder) else { return false };
    let files: Vec<(String, u64)> = entries
        .flatten()
        .filter_map(|entry| {
            let meta = entry.metadata().ok().filter(std::fs::Metadata::is_file)?;
            Some((entry.file_name().to_string_lossy().into_owned(), meta.len()))
        })
        .collect();
    NEEDED.iter().all(|list| {
        files.iter().any(|(name, _)| name == list)
            && files.iter().any(|(name, bytes)| *bytes > 0 && name.strip_prefix(list).and_then(|rest| rest.strip_prefix('.')).is_some_and(data_suffix))
    })
}

/// `4_13440274420601580`: a list's data file's suffix.
fn data_suffix(suffix: &str) -> bool {
    suffix.split_once('_').is_some_and(|(version, stamp)| {
        [version, stamp].iter().all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("safe-browsing-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lists(folder: &Path) {
        std::fs::create_dir_all(folder).unwrap();
        for (name, data) in [("UrlSoceng.store", "4_13440274420601580"), ("UrlMalware.store", "4_13440274420410634")] {
            std::fs::write(folder.join(name), b"index").unwrap();
            std::fs::write(folder.join(format!("{name}.{data}")), b"data").unwrap();
        }
    }

    /// Whose a seed is, is read before the namespace: a folder the person
    /// could have written (any test's own) isn't usable.
    #[test]
    fn only_a_seed_of_roots_alone_is_usable() {
        let dir = temp("usable");
        lists(&dir.join("seed"));
        assert!(!usable(&dir.join("seed")));
        assert!(!seed_in_place(&dir.join("seed")), "lists, but not root's");
        assert!(!usable(&dir.join("none")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A profile without lists gets the seed's; its own lists are never
    /// replaced, and an empty seed (a hidden one, in the namespace) or
    /// none leaves it without, for Chrome to fetch.
    #[test]
    fn only_a_profile_without_lists_gets_a_seed_and_only_one_with_lists() {
        let dir = temp("seed");
        let seed_dir = dir.join("seed");
        let user_data = dir.join("chrome");
        std::fs::create_dir_all(&user_data).unwrap();
        std::fs::create_dir_all(&seed_dir).unwrap();
        assert_eq!(seed(&user_data, &seed_dir), Ok(false), "an empty seed");
        assert_eq!(seed(&user_data, &dir.join("none")), Ok(false), "no seed here");
        assert!(!user_data.join(FOLDER).exists(), "nothing for Chrome to take as its own");

        lists(&seed_dir);
        assert!(!ready(&user_data));
        assert_eq!(seed(&user_data, &seed_dir), Ok(true));
        assert!(ready(&user_data));
        assert!(!user_data.join(".safe-browsing-seeding").exists());

        let other = dir.join("other");
        std::fs::create_dir_all(user_data.join(FOLDER)).unwrap();
        std::fs::write(user_data.join(FOLDER).join("UrlSoceng.store"), b"its own").unwrap();
        lists(&other);
        assert_eq!(seed(&user_data, &other), Ok(false), "it has its own");
        assert_eq!(std::fs::read(user_data.join(FOLDER).join("UrlSoceng.store")).unwrap(), b"its own");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Lists count once phishing's and malware's data are there, not at
    /// their small store files, which Chrome writes first.
    #[test]
    fn a_profile_has_lists_once_their_data_is_there() {
        let dir = temp("ready");
        let lists = dir.join(FOLDER);
        std::fs::create_dir_all(&lists).unwrap();
        assert!(!ready(&dir));
        std::fs::write(lists.join("UrlSoceng.store"), b"index").unwrap();
        std::fs::write(lists.join("UrlMalware.store"), b"index").unwrap();
        assert!(!ready(&dir), "no data yet");
        std::fs::write(lists.join("UrlSoceng.store.4_13440274420601580"), b"data").unwrap();
        std::fs::write(lists.join("UrlMalware.store.4_13440274420410634"), b"").unwrap();
        assert!(!ready(&dir), "malware's data is empty");
        std::fs::write(lists.join("UrlMalware.store.notdata"), b"data").unwrap();
        assert!(!ready(&dir), "not a data file's name");
        std::fs::write(lists.join("UrlMalware.store.4_13440274420410634"), b"data").unwrap();
        assert!(ready(&dir));
        std::fs::remove_file(lists.join("UrlSoceng.store")).unwrap();
        assert!(!ready(&dir), "a data file without its store file");
        std::fs::write(lists.join("UrlSoceng.store"), b"index").unwrap();
        assert!(ready(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// What's copied: folders and regular files, private, never a link.
    #[test]
    fn a_seed_is_copied_files_and_folders_only() {
        let dir = temp("copy");
        let from = dir.join("from");
        std::fs::create_dir_all(from.join("sub")).unwrap();
        std::fs::write(from.join("UrlSoceng.store"), b"lists").unwrap();
        std::fs::write(from.join("sub").join("x"), b"x").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", from.join("link")).unwrap();
        let to = dir.join("to");
        let mut total = 0;
        copy(&from, &to, 0, &mut total).unwrap();
        assert_eq!(std::fs::read(to.join("UrlSoceng.store")).unwrap(), b"lists");
        assert_eq!(std::fs::read(to.join("sub").join("x")).unwrap(), b"x");
        assert!(std::fs::symlink_metadata(to.join("link")).is_err(), "no link copied");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&to).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(total, 6);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
