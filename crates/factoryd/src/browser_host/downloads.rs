//! Downloads an errand made (#435 R9; Fugo 36912, 36913, 37568, 38000): the
//! host's half of `errands::downloads`. A finished download is read from
//! the runner over C1's channel 5 only while factoryd asked for that GUID
//! (`Control::DownloadRead`), counted and hashed here whatever the runner
//! says, kept in a store by its SHA-256 (`<home>/browser-downloads`, 0600),
//! named by Claktory (never the page's name as it is) and copied, inert,
//! into the errand agent's folder (`<home>/errands/<agent>/downloads/
//! <errand>/`): made afresh (create_new, O_NOFOLLOW), 0644, never through a
//! symlinked folder, quarantined on a Mac. A row in `browser_downloads`
//! says what it is and where it came from, for `errand_file`.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use rusqlite::{OptionalExtension, params};
use sha2::Digest;
use tokio::sync::mpsc;

use super::{control, view};
use crate::browser_runner::wire::{self, Control};
use crate::errands::downloads::{Download, Landed, Landing, StoredRow};
use crate::factory::Factory;

pub(crate) const SCHEMA: &str = "
CREATE TABLE browser_downloads (
    errand TEXT NOT NULL,
    -- The name Claktory gave it, in the errand agent's folder.
    name TEXT NOT NULL,
    agent TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    -- The errand's page it started from, and the host it came from.
    origin TEXT NOT NULL,
    file_host TEXT NOT NULL,
    -- '' or what could run ('an installer', 'a script', 'a program').
    flag TEXT NOT NULL,
    created INTEGER NOT NULL,
    PRIMARY KEY (errand, name)
);
";

/// The most of one download Claktory keeps.
const MOST_BYTES: u64 = 200 * 1024 * 1024;
/// The most one errand keeps, in files and in bytes.
const MOST_FILES: u64 = 50;
const MOST_ERRAND_BYTES: u64 = 1024 * 1024 * 1024;
/// The free space a download leaves at least.
const LEAVES_FREE: u64 = 5 * 1024 * 1024 * 1024;
/// How long the runner may go quiet in a read, and how long a whole read
/// may take.
const QUIET: Duration = Duration::from_secs(60);
const WHOLE: Duration = Duration::from_secs(10 * 60);
/// Downloads read at once.
const READS: usize = 3;

/// What channel 5 brings a read.
#[derive(Debug)]
pub(super) enum Piece {
    Bytes(Vec<u8>),
    End,
    /// The runner cancelled it: why, cleaned.
    Cancelled(String),
}

/// The reads under way, by GUID (its 16 bytes, as channel 5 names it).
pub(super) type Reads = HashMap<[u8; 16], mpsc::Sender<Piece>>;

/// Where the store is.
fn store(factory: &Factory) -> PathBuf {
    factory.home.root().join("browser-downloads")
}

/// A channel 5 frame: to its read if factoryd asked for that GUID and
/// still reads it, else dropped unread.
pub(super) async fn chunk(one: &super::One, payload: Vec<u8>) {
    let Some(guid) = payload.get(..16).and_then(|guid| <[u8; 16]>::try_from(guid).ok()) else { return };
    let Some(read) = one.inner().reads.get(&guid).cloned() else { return };
    let piece = if payload.len() == 16 { Piece::End } else { Piece::Bytes(payload[16..].to_vec()) };
    // A read that stopped listening: it's ended anyway.
    let _ = tokio::time::timeout(QUIET, read.send(piece)).await;
}

/// The runner cancelled `guid`: its read, if any, hears why.
pub(super) fn cancelled(inner: &mut super::Inner, guid: &str, why: &str) {
    let Some(bytes) = wire::guid_bytes(guid) else { return };
    if let Some(read) = inner.reads.remove(&bytes) {
        let _ = read.try_send(Piece::Cancelled(view::cleaned(why, 200)));
    }
}

/// Deletes the runner's copy of `guid`, read or not.
pub(super) fn drop_download(one: &super::One, guid: &str) {
    let Some(bytes) = wire::guid_bytes(guid) else { return };
    let mut inner = one.inner();
    inner.reads.remove(&bytes);
    control(&inner, &Control::DownloadDrop { guid: guid.to_owned() });
}

/// Lands `download`: read, counted, hashed, kept and copied into its errand
/// agent's folder; the runner's copy deleted either way.
pub(super) fn land(factory: Weak<Factory>, one: Arc<super::One>, download: Download) -> Landing {
    Box::pin(async move {
        let factory = factory.upgrade().ok_or_else(|| "Claktory is stopping.".to_owned())?;
        let landed = tokio::time::timeout(WHOLE, landing(&factory, &one, &download))
            .await
            .unwrap_or_else(|_| Err("The download took more than 10 minutes to read, so Claktory didn't keep it.".to_owned()));
        if let Some(bytes) = wire::guid_bytes(&download.guid) {
            one.inner().reads.remove(&bytes);
        }
        drop_download(&one, &download.guid);
        landed
    })
}

async fn landing(factory: &Arc<Factory>, one: &super::One, download: &Download) -> Result<Landed, String> {
    let guid = wire::guid_bytes(&download.guid).ok_or_else(|| "That isn't a download of Claktory's browser.".to_owned())?;
    if !id_ok(&download.agent) || !download.errand.starts_with("errand-") || !id_ok(&download.errand) {
        return Err("That isn't an errand's download.".to_owned());
    }
    // The errand's own downloads so far, and the room left.
    let (files, bytes) = factory
        .store
        .db()
        .query_row("SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM browser_downloads WHERE errand = ?1", params![download.errand], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| error.to_string())?;
    let (files, so_far) = (files.unsigned_abs(), bytes.unsigned_abs());
    if files >= MOST_FILES {
        return Err(format!("This errand has kept {MOST_FILES} downloads, the most one errand keeps."));
    }
    let room = MOST_BYTES.min(MOST_ERRAND_BYTES.saturating_sub(so_far));
    let store = store(factory);
    make_private_folder(&store)?;
    if free(&store).is_some_and(|free| free < LEAVES_FREE + room.min(MOST_BYTES)) {
        return Err("This computer is short of space, so Claktory keeps no more downloads.".to_owned());
    }
    // Asked for: only now does channel 5 bring it here.
    let (send, mut pieces) = mpsc::channel(4);
    {
        let mut inner = one.inner();
        if one.agent != download.agent || inner.errand != download.errand || inner.runner.is_none() {
            return Err("Claktory's browser isn't doing this errand now.".to_owned());
        }
        if inner.reads.len() >= READS || inner.reads.contains_key(&guid) {
            return Err("Claktory's browser is reading other downloads: try again in a moment.".to_owned());
        }
        inner.reads.insert(guid, send);
        control(&inner, &Control::DownloadRead { guid: download.guid.clone() });
    }
    let temporary = store.join(format!(".{}.part", download.guid));
    let _ = std::fs::remove_file(&temporary);
    let mut file = private_file(&temporary)?;
    let read = read(&mut pieces, &mut file, room, so_far).await;
    drop(file);
    let (count, sha256, head) = match read {
        Ok(read) => read,
        Err(why) => {
            let _ = std::fs::remove_file(&temporary);
            return Err(why);
        }
    };
    let kept = store.join(&sha256);
    if kept.is_file() {
        let _ = std::fs::remove_file(&temporary);
    } else {
        std::fs::rename(&temporary, &kept).map_err(|error| format!("Claktory couldn't keep the download: {error}"))?;
    }
    let name = name_for(&download.suggested, &download.url);
    let flag = flag_for(&name, &head);
    let folder = factory.home.root().join("errands").join(&download.agent).join("downloads").join(&download.errand);
    let path = copy_into(factory.home.root(), &kept, &folder, &name)?;
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or(name);
    let file_host = url::Url::parse(&download.url).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_default();
    factory
        .store
        .db()
        .execute(
            "INSERT OR REPLACE INTO browser_downloads (errand, name, agent, sha256, bytes, origin, file_host, flag, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                download.errand,
                name,
                download.agent,
                sha256,
                i64::try_from(count).unwrap_or(i64::MAX),
                view::cleaned(&download.page_origin, 200),
                view::cleaned(&file_host, 253),
                flag,
                crate::transcript::now_ms()
            ],
        )
        .map_err(|error| error.to_string())?;
    tracing::info!(errand = %download.errand, bytes = count, %flag, "kept an errand's download");
    Ok(Landed { name, path, bytes: count, sha256, flag })
}

/// Reads the pieces into `file`: its size, SHA-256 and first bytes.
async fn read(
    pieces: &mut mpsc::Receiver<Piece>,
    file: &mut std::fs::File,
    room: u64,
    so_far: u64,
) -> Result<(u64, String, Vec<u8>), String> {
    let mut hasher = sha2::Sha256::new();
    let mut count: u64 = 0;
    let mut head = Vec::new();
    loop {
        let piece = tokio::time::timeout(QUIET, pieces.recv()).await.ok().flatten();
        match piece {
            Some(Piece::Bytes(bytes)) => {
                count += bytes.len() as u64;
                if count > room {
                    return Err(if so_far + count > MOST_ERRAND_BYTES && count <= MOST_BYTES {
                        "This errand's downloads would pass 1 GB, the most one errand keeps.".to_owned()
                    } else {
                        "It's larger than 200 MB, the most Claktory keeps of one download.".to_owned()
                    });
                }
                if head.len() < 8 {
                    head.extend(bytes.iter().take(8 - head.len()));
                }
                hasher.update(&bytes);
                file.write_all(&bytes).map_err(|error| format!("Claktory couldn't keep the download: {error}"))?;
            }
            Some(Piece::End) => break,
            Some(Piece::Cancelled(why)) => return Err(format!("Claktory's browser cancelled the download: {why}.")),
            None => return Err("The download stopped before it was all read.".to_owned()),
        }
    }
    file.sync_all().map_err(|error| format!("Claktory couldn't keep the download: {error}"))?;
    Ok((count, crate::encoding::hex(&hasher.finalize()), head))
}

/// `errand`'s landed download `name`: the store's file, and its row; found
/// by the row, never by joining `name` onto a path.
pub(super) fn stored(factory: &Factory, errand: &str, name: &str) -> Option<(PathBuf, StoredRow)> {
    let row = factory
        .store
        .db()
        .query_row(
            "SELECT sha256, bytes, origin, file_host, flag FROM browser_downloads WHERE errand = ?1 AND name = ?2",
            params![errand, name],
            |row| {
                Ok(StoredRow {
                    sha256: row.get(0)?,
                    bytes: row.get::<_, i64>(1)?.unsigned_abs(),
                    origin: row.get(2)?,
                    file_host: row.get(3)?,
                    flag: row.get(4)?,
                })
            },
        )
        .optional()
        .ok()
        .flatten()?;
    if row.sha256.len() != 64 || !row.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let file = store(factory).join(&row.sha256);
    file.is_file().then_some((file, row))
}

/// Copies `file` into `folder` (under `root`, Claktory's folder) as `name`,
/// or as "name (2)" and on when it's taken: made afresh, never through a
/// symlink, 0644 with no execute bit. The copy's path, or why not.
pub(crate) fn copy_into(root: &Path, file: &Path, folder: &Path, name: &str) -> Result<PathBuf, String> {
    if !name_ok(name) {
        return Err("That isn't a name Claktory gave a download.".to_owned());
    }
    // The folder held open: the copy is made in it, never by its path, so
    // nothing swapped in meanwhile redirects it (Fugo 38172).
    let held = real_folder(root, folder)?;
    let mut source = std::fs::File::open(file).map_err(|error| format!("Claktory couldn't read the download: {error}"))?;
    let (stem, extension) = split_name(name);
    for attempt in 1..=99 {
        let called = if attempt == 1 { name.to_owned() } else { format!("{stem} ({attempt}){extension}") };
        let path = folder.join(&called);
        let mut copy = match create_in(&held, &called) {
            Ok(copy) => copy,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Claktory couldn't copy the download: {error}")),
        };
        std::io::copy(&mut source, &mut copy).map_err(|error| format!("Claktory couldn't copy the download: {error}"))?;
        copy.sync_all().map_err(|error| format!("Claktory couldn't copy the download: {error}"))?;
        quarantine(&path);
        return Ok(path);
    }
    Err(format!("There are too many downloads called {name} there already."))
}

/// Opens `folder`, under `root`, one folder at a time from `root`, each
/// part opened without following a symlink (one made in an agent's folder
/// would send the copy elsewhere) and made when it isn't there: the last
/// one, held open.
#[cfg(unix)]
fn real_folder(root: &Path, folder: &Path) -> Result<std::os::fd::OwnedFd, String> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    let refused = || "That isn't a folder Claktory keeps downloads in.".to_owned();
    let made = |error: std::io::Error| format!("Claktory couldn't make the folder for downloads: {error}");
    let rest = folder.strip_prefix(root).map_err(|_| refused())?;
    let mut held: OwnedFd = std::fs::File::open(root).map_err(made)?.into();
    for part in rest.components() {
        let std::path::Component::Normal(part) = part else { return Err(refused()) };
        let part = std::ffi::CString::new(part.as_bytes()).map_err(|_| refused())?;
        let open = |at: &OwnedFd| {
            // SAFETY: a valid C string, relative to a folder we hold open.
            unsafe { libc::openat(at.as_raw_fd(), part.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) }
        };
        let mut fd = open(&held);
        if fd < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
            // SAFETY: as above.
            if unsafe { libc::mkdirat(held.as_raw_fd(), part.as_ptr(), 0o755) } != 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::EEXIST) {
                    return Err(made(error));
                }
            }
            fd = open(&held);
        }
        if fd < 0 {
            return Err(refused());
        }
        // SAFETY: a descriptor openat just gave us, owned from here.
        held = unsafe { OwnedFd::from_raw_fd(fd) };
    }
    Ok(held)
}

/// Not on Windows, where Claktory's browser doesn't run yet (`setup`): no
/// folder is held, so nothing is copied.
#[cfg(windows)]
fn real_folder(_root: &Path, _folder: &Path) -> Result<std::convert::Infallible, String> {
    Err("Downloads from Claktory's browser aren't kept on Windows yet.".to_owned())
}

#[cfg(windows)]
fn create_in(folder: &std::convert::Infallible, _name: &str) -> std::io::Result<std::fs::File> {
    match *folder {}
}

/// Makes `name` in the folder held: afresh (O_EXCL), never through a
/// symlink (O_NOFOLLOW), 0644.
#[cfg(unix)]
fn create_in(folder: &std::os::fd::OwnedFd, name: &str) -> std::io::Result<std::fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let name = std::ffi::CString::new(name).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: a valid C string, relative to a folder we hold open.
    let fd = unsafe { libc::openat(folder.as_raw_fd(), name.as_ptr(), flags, 0o644 as libc::c_uint) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a descriptor openat just gave us, owned from here.
    Ok(std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) }))
}

fn make_private_folder(folder: &Path) -> Result<(), String> {
    #[cfg(windows)]
    factory_agent::windows::private::create_dir_all(folder).map_err(|error| format!("Claktory couldn't make its downloads' folder: {error}"))?;
    std::fs::create_dir_all(folder).map_err(|error| format!("Claktory couldn't make its downloads' folder: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn private_file(path: &Path) -> Result<std::fs::File, String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path).map_err(|error| format!("Claktory couldn't keep the download: {error}"))
}

/// Free bytes on the disk `path` is on.
#[cfg(windows)]
fn free(path: &Path) -> Option<u64> {
    crate::windows::fs::space(path).ok().map(|space| space.available)
}

/// Free bytes on the disk `path` is on.
#[cfg(unix)]
fn free(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid C string and a statvfs of our own.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

/// A Claktory or errand id: letters, digits, '-' and '_'.
fn id_ok(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Whether `name` is one Claktory could have given: one part, plain.
fn name_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 255 && name_for(name, "") == name && !matches!(name, "." | "..") && !name.contains(['/', '\\'])
}

/// The name Claktory gives a download: the page's (or its address's last
/// part), its last part only, cleaned (no hidden or control characters,
/// nothing a shell or a path reads specially, no leading dot), at most 120
/// characters with its extension kept; "download" when nothing's left.
pub(crate) fn name_for(suggested: &str, url: &str) -> String {
    let from_url = || url::Url::parse(url).ok().and_then(|url| url.path_segments()?.next_back().map(str::to_owned)).unwrap_or_default();
    let suggested = if suggested.trim().is_empty() { from_url() } else { suggested.to_owned() };
    let last = suggested.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = view::cleaned(last, 400)
        .chars()
        .map(|char| if char.is_alphanumeric() || " .-_()+,[]".contains(char) { char } else { '_' })
        .collect();
    // No leading '-' either: a name passed to a command never reads as an
    // option (Fugo 38172).
    let cleaned = cleaned.trim_start_matches(['.', ' ', '-']).trim_end_matches(['.', ' ']).to_owned();
    if cleaned.is_empty() {
        return "download".to_owned();
    }
    let (stem, extension) = split_name(&cleaned);
    let extension: String = if extension.chars().count() <= 11 { extension.to_owned() } else { String::new() };
    let room = 120usize.saturating_sub(extension.chars().count());
    let stem: String = stem.chars().take(room).collect::<String>().trim_end().to_owned();
    if stem.is_empty() { format!("download{extension}") } else { format!("{stem}{extension}") }
}

/// A name's stem and extension (".zip", or "").
fn split_name(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    }
}

/// What a download is that could run, by its extension or its first bytes:
/// kept all the same, never executable.
pub(crate) fn flag_for(name: &str, head: &[u8]) -> String {
    let extension = split_name(name).1.trim_start_matches('.').to_ascii_lowercase();
    const INSTALLERS: &[&str] = &["dmg", "pkg", "mpkg", "exe", "msi", "deb", "rpm", "apk", "appimage", "snap", "flatpak"];
    const SCRIPTS: &[&str] =
        &["sh", "bash", "zsh", "command", "bat", "cmd", "ps1", "py", "rb", "pl", "js", "vbs", "applescript", "scpt", "jar"];
    let program = head.starts_with(b"\x7fELF")
        || head.starts_with(b"MZ")
        || head.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
        || head.starts_with(&[0xce, 0xfa, 0xed, 0xfe])
        || head.starts_with(&[0xca, 0xfe, 0xba, 0xbe])
        || head.starts_with(b"#!");
    if INSTALLERS.contains(&extension.as_str()) {
        "an installer".to_owned()
    } else if SCRIPTS.contains(&extension.as_str()) || head.starts_with(b"#!") {
        "a script".to_owned()
    } else if program {
        "a program".to_owned()
    } else {
        String::new()
    }
}

/// Marks a copy as downloaded, so macOS asks before anything opens it.
#[cfg(target_os = "macos")]
fn quarantine(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return };
    let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs());
    let value = format!("0083;{seconds:x};Claktory;");
    // SAFETY: valid C strings and a buffer of the length given.
    unsafe {
        libc::setxattr(path.as_ptr(), c"com.apple.quarantine".as_ptr(), value.as_ptr().cast(), value.len(), 0, 0);
    }
}

#[cfg(not(target_os = "macos"))]
fn quarantine(_path: &Path) {}

// Claktory's browser isn't on Windows (`setup::check` says so).
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_download_is_named_by_claktory() {
        assert_eq!(name_for("report.pdf", ""), "report.pdf");
        assert_eq!(name_for("../../etc/passwd", ""), "passwd");
        assert_eq!(name_for("..\\..\\boot.ini", ""), "boot.ini");
        assert_eq!(name_for(".bashrc", ""), "bashrc");
        assert_eq!(name_for("evil\u{202e}fdp.exe", ""), "evilfdp.exe");
        assert_eq!(name_for("a;rm -rf $HOME`.txt", ""), "a_rm -rf _HOME_.txt");
        assert_eq!(name_for("", "https://example.com/files/data%20set.csv?x=1"), "data_20set.csv");
        assert_eq!(name_for("", "https://example.com/"), "download");
        assert_eq!(name_for("...", ""), "download");
        assert_eq!(name_for("-rf.txt", ""), "rf.txt");
        let long = format!("{}.zip", "x".repeat(300));
        assert_eq!(name_for(&long, "").chars().count(), 120);
        assert!(name_for(&long, "").ends_with(".zip"));
        assert!(name_ok("report.pdf") && name_ok("run (2).sh"));
        assert!(!name_ok("../x") && !name_ok(".x") && !name_ok("a/b") && !name_ok("") && !name_ok("a\u{202e}b"));
    }

    #[test]
    fn what_could_run_is_flagged() {
        assert_eq!(flag_for("setup.dmg", b""), "an installer");
        assert_eq!(flag_for("run.sh", b"echo"), "a script");
        assert_eq!(flag_for("notes.txt", b"#!/bin/sh"), "a script");
        assert_eq!(flag_for("tool", b"\x7fELF\x02"), "a program");
        assert_eq!(flag_for("tool.bin", &[0xcf, 0xfa, 0xed, 0xfe]), "a program");
        assert_eq!(flag_for("report.pdf", b"%PDF-1.7"), "");
    }

    #[test]
    fn a_copy_is_made_afresh_and_never_through_a_symlink() {
        let root = std::env::temp_dir().join(format!("claktory-downloads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("errands/runner")).unwrap();
        let file = root.join("stored");
        std::fs::write(&file, b"hello").unwrap();
        let folder = root.join("errands/runner/downloads/errand-1");
        let first = copy_into(&root, &file, &folder, "a.txt").unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"hello");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&first).unwrap().permissions().mode() & 0o777, 0o644);
        }
        assert_eq!(copy_into(&root, &file, &folder, "a.txt").unwrap(), folder.join("a (2).txt"));
        // A symlink where the next one goes is skipped, never written through.
        let outside = root.join("outside");
        std::os::unix::fs::symlink(&outside, folder.join("a (3).txt")).unwrap();
        assert_eq!(copy_into(&root, &file, &folder, "a.txt").unwrap(), folder.join("a (4).txt"));
        assert!(!outside.exists());
        // A symlinked folder is refused.
        std::fs::create_dir_all(root.join("errands/other")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join("errands/other/downloads")).unwrap();
        assert!(copy_into(&root, &file, &root.join("errands/other/downloads/errand-1"), "a.txt").is_err());
        // Outside Claktory's folder, or a name it never gives: refused.
        assert!(copy_into(&root, &file, Path::new("/tmp/x"), "a.txt").is_err());
        assert!(copy_into(&root, &file, &folder, "../a.txt").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
