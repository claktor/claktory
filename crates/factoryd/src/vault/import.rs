//! Files the vault imports from this computer (secrets_request's `import`):
//! an agent names a file, the person approves on a card that shows exactly
//! which file it is (never its contents), and factoryd reads the bytes
//! itself and seals them straight into a new secret's file variable,
//! binary files too. No Terminal, no clipboard.
//!
//! The agent picks the path, so the file is held to more than its name:
//! - no symbolic link anywhere in the path (each component's lstat), opened
//!   with O_NOFOLLOW, and a regular file by its fstat, at most 1 MiB;
//! - never Claktory's own homes, /dev, /proc or /sys;
//! - at approval, the same file (device and inode), the same size and
//!   modification time, and the same contents (`fingerprint`), or it's
//!   refused as changed;
//! - its fingerprint is keyed with a key derived from the vault's own, so
//!   showing it never helps guess a weak password inside a known template.

use std::io::Read as _;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use base64::Engine;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::Zeroizing;

use super::{Vault, VaultError, crypto};

/// The largest file imported.
pub(crate) const MOST_BYTES: u64 = 1024 * 1024;
/// The most files one request imports.
pub(crate) const MOST_FILES: usize = 10;
/// An imported secret's kind.
pub(crate) const KIND_IMPORT: &str = "import";
/// What the fingerprint's key is derived for.
const PURPOSE: &str = "claktory vault import fingerprint v1";

/// A file as asked for, kept with the request: what was read of it then.
/// Holds no contents; `keyed` is the full keyed fingerprint.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Asked {
    pub variable: String,
    pub path: String,
    pub size: u64,
    pub modified_ms: i64,
    pub dev: u64,
    pub ino: u64,
    pub keyed: String,
}

/// What an agent names: a variable and a file.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Wanted {
    pub variable: String,
    pub path: String,
}

fn refused(why: impl Into<String>) -> VaultError {
    VaultError::Invalid(why.into())
}

/// The person's home folder.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// `raw` as an absolute path: absolute already, or `~/…`; never with `.`
/// or `..` in it.
pub(crate) fn absolute(raw: &str) -> Result<PathBuf, VaultError> {
    let raw = raw.trim();
    let path = match raw.strip_prefix("~/") {
        Some(rest) => home_dir().ok_or_else(|| refused("This computer's home folder isn't known."))?.join(rest),
        None => PathBuf::from(raw),
    };
    if !path.is_absolute() {
        return Err(refused(format!("Give the file's full path, starting with / or ~/, not \"{raw}\".")));
    }
    if path.components().any(|component| matches!(component, Component::CurDir | Component::ParentDir)) {
        return Err(refused(format!("Give the file's own path, without . or .. in it: \"{raw}\".")));
    }
    // Rebuilt from its parts: no doubled or trailing slashes.
    Ok(path.components().collect())
}

/// Where a file must never come from: Claktory's own homes (`homes`) and
/// the system's device and process trees.
fn forbidden(path: &Path, homes: &[PathBuf]) -> Option<String> {
    for root in ["/dev", "/proc", "/sys"] {
        if path.starts_with(root) {
            return Some(format!("Claktory doesn't import from {root}."));
        }
    }
    homes
        .iter()
        .any(|home| path.starts_with(home))
        .then(|| "That's inside Claktory's own folder, where its vault and settings are: Claktory doesn't import from there.".to_owned())
}

/// Claktory's homes: this one, and the real and dev ones in the home folder.
/// Each as given and, where it differs, as it really is (a path asked for
/// has no link in it, so it's compared with the real one).
pub(crate) fn homes(this: &Path) -> Vec<PathBuf> {
    let mut given = vec![this.to_path_buf()];
    if let Some(home) = home_dir() {
        given.push(home.join(".factory"));
        given.push(home.join(".factory-dev"));
    }
    let mut homes = given.clone();
    homes.extend(given.iter().filter_map(|home| home.canonicalize().ok()).filter(|real| !given.contains(real)));
    homes
}

/// No symbolic link anywhere in `path`: each of its folders, and itself.
fn no_links(path: &Path) -> Result<(), VaultError> {
    let mut so_far = PathBuf::new();
    for component in path.components() {
        so_far.push(component);
        if matches!(component, Component::RootDir) {
            continue;
        }
        match std::fs::symlink_metadata(&so_far) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let target = std::fs::read_link(&so_far).map(|target| format!(" (to {})", target.display())).unwrap_or_default();
                return Err(refused(format!(
                    "{} is a symbolic link{target}. Give the path of the file itself, with no link in it.",
                    so_far.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(refused(format!("There's no file at {}.", path.display())));
            }
            Err(error) => return Err(refused(format!("{} can't be read: {error}", so_far.display()))),
        }
    }
    Ok(())
}

/// A file opened and checked, and what was read of it.
pub(crate) struct Opened {
    pub size: u64,
    pub modified_ms: i64,
    pub dev: u64,
    pub ino: u64,
    pub bytes: Zeroizing<Vec<u8>>,
}

/// Never the bytes.
impl std::fmt::Debug for Opened {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Opened").field("size", &self.size).field("dev", &self.dev).field("ino", &self.ino).finish_non_exhaustive()
    }
}

/// Opens `path` as it must be to import: through no link, never one of
/// `homes`, a regular file of at most 1 MiB; reads it.
pub(crate) fn read(path: &Path, homes: &[PathBuf]) -> Result<Opened, VaultError> {
    if let Some(why) = forbidden(path, homes) {
        return Err(refused(why));
    }
    no_links(path)?;
    // No following a link put there since, in any folder or as the file,
    // and no waiting on a FIFO.
    let file = open_walked(path).map_err(|error| refused(format!("{} can't be opened: {error}", path.display())))?;
    let meta = file.metadata().map_err(|error| refused(format!("{} can't be read: {error}", path.display())))?;
    if !meta.file_type().is_file() {
        return Err(refused(format!("{} isn't a file: only a regular file can be imported.", path.display())));
    }
    // Another name for the same file would be another path than the card's.
    if meta.nlink() != 1 {
        return Err(refused(format!(
            "{} has other names on this computer (hard links), so it can't be shown for sure which file it is. Copy it to a file of its own and ask for that.",
            path.display()
        )));
    }
    if meta.len() > MOST_BYTES {
        return Err(refused(format!("{} is over 1 MB, too big to keep as a secret.", path.display())));
    }
    let mut bytes = Zeroizing::new(Vec::with_capacity(meta.len() as usize));
    (&file).take(MOST_BYTES + 1).read_to_end(&mut bytes).map_err(|error| refused(format!("{} can't be read: {error}", path.display())))?;
    if bytes.len() as u64 > MOST_BYTES {
        return Err(refused(format!("{} is over 1 MB, too big to keep as a secret.", path.display())));
    }
    Ok(Opened { size: bytes.len() as u64, modified_ms: modified_ms(&meta), dev: meta.dev(), ino: meta.ino(), bytes })
}

/// Opens `path` (absolute, plain) one folder at a time from `/`, each with
/// openat and O_NOFOLLOW, so no link is followed anywhere in it, even one
/// swapped in after `no_links` looked: what's opened is the file at this
/// very path.
fn open_walked(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    let open = |dir: Option<&OwnedFd>, name: &std::ffi::CStr, flags: libc::c_int| -> std::io::Result<OwnedFd> {
        // SAFETY: a valid C string and, when given, an open folder; the
        // descriptor returned is owned here.
        let fd = unsafe {
            match dir {
                Some(dir) => libc::openat(dir.as_raw_fd(), name.as_ptr(), flags),
                None => libc::open(name.as_ptr(), flags),
            }
        };
        if fd < 0 { Err(std::io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(fd) }) }
    };
    let folder = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let mut dir = open(None, c"/", folder)?;
    let names: Vec<&std::ffi::OsStr> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    let Some((file, folders)) = names.split_last() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a file's path"));
    };
    let c_name = |name: &std::ffi::OsStr| {
        std::ffi::CString::new(name.as_bytes()).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "a NUL in the path"))
    };
    for name in folders {
        dir = open(Some(&dir), &c_name(name)?, folder)?;
    }
    let file = open(Some(&dir), &c_name(file)?, libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)?;
    Ok(std::fs::File::from(file))
}

fn modified_ms(meta: &std::fs::Metadata) -> i64 {
    meta.mtime() * 1000 + meta.mtime_nsec() / 1_000_000
}

/// Why the file asked for can't be imported now, from what's there (no
/// reading): gone, or another file or changed since it was asked for.
/// Empty when it looks the same.
pub(crate) fn problem(asked: &Asked) -> String {
    match std::fs::symlink_metadata(&asked.path) {
        Err(_) => "The file isn't there any more.".to_owned(),
        Ok(meta) if meta.file_type().is_symlink() || !meta.file_type().is_file() => {
            "It isn't the same file any more: it's been replaced.".to_owned()
        }
        Ok(meta) if (meta.dev(), meta.ino()) != (asked.dev, asked.ino) => "It isn't the same file any more: it's been replaced.".to_owned(),
        Ok(meta) if meta.len() != asked.size || modified_ms(&meta) != asked.modified_ms => {
            "The file changed since it was asked for.".to_owned()
        }
        Ok(_) => String::new(),
    }
}

/// How a keyed fingerprint shows: its first 12 hex digits, by fours.
pub(crate) fn shown(keyed: &str) -> String {
    keyed.chars().take(12).collect::<Vec<_>>().chunks(4).map(|four| four.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
}

/// The kind of sensitive place `path` is in, for the card ("ssh"…), or "".
pub(crate) fn sensitive(path: &Path) -> &'static str {
    let Some(home) = home_dir() else { return "" };
    let under = |relative: &str| path.starts_with(home.join(relative));
    let places: [(&[&str], &'static str); 8] = [
        (&[".ssh"], "ssh"),
        (&[".gnupg"], "gnupg"),
        (&[".aws"], "aws"),
        (&[".config/gcloud"], "gcloud"),
        (&[".kube"], "kube"),
        (&[".docker"], "docker"),
        (&["Library/Keychains"], "keychains"),
        (
            &[
                "Library/Application Support/Google/Chrome",
                "Library/Application Support/Chromium",
                "Library/Application Support/BraveSoftware",
                "Library/Application Support/Microsoft Edge",
                "Library/Application Support/Firefox",
                "Library/Application Support/Arc",
                "Library/Safari",
                ".mozilla",
                ".config/google-chrome",
                ".config/chromium",
                ".config/BraveSoftware",
            ],
            "browser",
        ),
    ];
    places.iter().find(|(folders, _)| folders.iter().any(|folder| under(folder))).map_or("", |(_, kind)| kind)
}

/// A file's bytes as the vault keeps them: text as it is, anything else as
/// standard base64 (binary).
pub(crate) fn value(bytes: &[u8]) -> (Zeroizing<String>, bool) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (Zeroizing::new(text.to_owned()), false),
        Err(_) => (Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(bytes)), true),
    }
}

impl Vault {
    /// `bytes`' keyed fingerprint, hex: HMAC-SHA256 under a key derived
    /// from the vault's own. Needs the vault open.
    pub(crate) fn import_fingerprint(&self, bytes: &[u8]) -> Result<String, VaultError> {
        let inner = self.lock();
        let vmk = Self::open_vmk(&inner)?;
        let key = crypto::derive(vmk.as_slice(), inner.vault_id.as_bytes(), PURPOSE);
        let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key.as_slice())
            .map_err(|_| refused("The vault couldn't fingerprint the file."))?;
        mac.update(bytes);
        Ok(crypto::hex(&mac.finalize().into_bytes()))
    }

    /// Reads and checks each file an agent asks to import: what's kept with
    /// the request. Refused with the reason when any can't be imported.
    pub(crate) fn asked_imports(&self, wanted: &[Wanted], homes: &[PathBuf]) -> Result<Vec<Asked>, VaultError> {
        if wanted.len() > MOST_FILES {
            return Err(refused(format!("A request imports at most {MOST_FILES} files.")));
        }
        let mut asked: Vec<Asked> = Vec::new();
        for file in wanted {
            let variable = file.variable.trim().to_owned();
            if !super::valid_variable(&variable) {
                return Err(refused(format!(
                    "\"{variable}\" can't be a variable's name: use letters, digits and '_', not starting with a digit."
                )));
            }
            if asked.iter().any(|other| other.variable == variable) {
                return Err(refused(format!("{variable} is there twice.")));
            }
            let path = absolute(&file.path)?;
            let Some(text) = path.to_str().map(str::to_owned) else {
                return Err(refused("Claktory can't import a file whose path isn't text."));
            };
            if asked.iter().any(|other| other.path == text) {
                return Err(refused(format!("{text} is there twice.")));
            }
            let read = read(&path, homes)?;
            let keyed = self.import_fingerprint(&read.bytes)?;
            asked.push(Asked { variable, path: text, size: read.size, modified_ms: read.modified_ms, dev: read.dev, ino: read.ino, keyed });
        }
        Ok(asked)
    }

    /// Each file again, at approval: the same file, unchanged, or refused
    /// as changed. The bytes to seal.
    pub(crate) fn approved_imports(&self, asked: &[Asked], homes: &[PathBuf]) -> Result<Vec<Opened>, VaultError> {
        let mut reads = Vec::with_capacity(asked.len());
        for file in asked {
            let changed =
                || refused(format!("{} changed since it was asked for, so it wasn't imported. The agent can ask for it again.", file.path));
            let read = read(Path::new(&file.path), homes)?;
            if (read.dev, read.ino, read.size, read.modified_ms) != (file.dev, file.ino, file.size, file.modified_ms) {
                return Err(changed());
            }
            if self.import_fingerprint(&read.bytes)? != file.keyed {
                return Err(changed());
            }
            reads.push(read);
        }
        Ok(reads)
    }

    /// Marks secret `id` as imported: its values can't be edited, only
    /// imported again.
    pub(crate) fn mark_imported(&self, id: &str) -> Result<(), VaultError> {
        let mut inner = self.lock();
        Self::db(&inner)?.execute("UPDATE secrets SET kind = ?2 WHERE id = ?1", [id, KIND_IMPORT])?;
        inner.state = None;
        Ok(())
    }

    /// Records the import in the secret's history: `agent` asked, `detail`
    /// says what came from where.
    pub(crate) fn log_import(&self, id: &str, agent: &str, detail: &str) -> Result<(), VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let (id, name) = Self::secret_id(db, id)?;
        Self::log(db, &id, &name, "import", agent, "", detail)?;
        inner.state = None;
        Ok(())
    }
}

/// Deletes the original at `asked.path` once it's in the vault, if it's
/// still the same file (device and inode): a plain delete. Whether it was.
pub(crate) fn delete_original(asked: &Asked) -> bool {
    match std::fs::symlink_metadata(&asked.path) {
        Ok(meta) if meta.file_type().is_file() && (meta.dev(), meta.ino()) == (asked.dev, asked.ino) => {
            std::fs::remove_file(&asked.path).is_ok()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_absolute_and_plain() {
        assert!(absolute("relative/file").is_err());
        assert!(absolute("/a/../etc/passwd").is_err());
        assert_eq!(absolute("/a//b/").expect("plain"), PathBuf::from("/a/b"));
        assert_eq!(shown("3f9a21c07d44e1ff"), "3f9a 21c0 7d44");
        assert!(!value(b"text\n").1);
        assert!(value(&[0xff, 0x00, 0x80]).1);
    }

    #[test]
    fn a_file_is_read_only_as_itself() {
        let dir = std::env::temp_dir().join(format!("import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real")).expect("dir");
        let dir = dir.canonicalize().expect("canonical");
        let file = dir.join("real/key.p12");
        std::fs::write(&file, [0u8, 1, 2, 255]).expect("written");
        let none: Vec<PathBuf> = Vec::new();
        assert_eq!(read(&file, &none).expect("read").bytes.as_slice(), [0u8, 1, 2, 255]);
        // A link as the file, or in the middle.
        std::os::unix::fs::symlink(&file, dir.join("link.p12")).expect("link");
        std::os::unix::fs::symlink(dir.join("real"), dir.join("folder")).expect("link");
        assert!(read(&dir.join("link.p12"), &none).unwrap_err().to_string().contains("is a symbolic link"));
        assert!(read(&dir.join("folder/key.p12"), &none).unwrap_err().to_string().contains("is a symbolic link"));
        // A folder, a FIFO, one byte over 1 MiB.
        assert!(read(&dir.join("real"), &none).unwrap_err().to_string().contains("isn't a file"));
        let fifo = dir.join("fifo");
        let made = std::ffi::CString::new(fifo.to_str().expect("text")).expect("c");
        assert_eq!(unsafe { libc::mkfifo(made.as_ptr(), 0o600) }, 0);
        assert!(read(&fifo, &none).unwrap_err().to_string().contains("isn't a file"));
        let big = dir.join("big");
        std::fs::write(&big, vec![b'a'; MOST_BYTES as usize + 1]).expect("written");
        assert!(read(&big, &none).unwrap_err().to_string().contains("over 1 MB"));
        // A hard link: the same file by another name.
        std::fs::hard_link(&file, dir.join("other.p12")).expect("hard link");
        assert!(read(&dir.join("other.p12"), &none).unwrap_err().to_string().contains("hard links"));
        assert!(read(&file, &none).unwrap_err().to_string().contains("hard links"));
        std::fs::remove_file(dir.join("other.p12")).expect("removed");
        assert!(read(&file, &none).is_ok());
        // Claktory's own folder.
        assert!(read(&file, std::slice::from_ref(&dir)).unwrap_err().to_string().contains("Claktory's own folder"));
        assert!(read(Path::new("/dev/null"), &none).unwrap_err().to_string().contains("/dev"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
