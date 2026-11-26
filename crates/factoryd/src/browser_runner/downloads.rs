//! Downloads, the runner's half (agent-browser.md R9, Fugo 36912, 36914):
//! a driven errand's files land in `<the session's folder>/downloads/`,
//! named by Chrome after their GUID (factoryd sets `allowAndName` there,
//! with the path [`Downloads::dir`] gives in Ready). The runner reads
//! Chrome's download events as they pass on their way to factoryd and
//! holds the caps itself while Chrome writes, since that folder is on this
//! computer's disk: 200 MB a file, 3 at once, 5 GiB left free. It sends a
//! finished file only when asked (`download_read`), from the folder's own
//! descriptor, never following a link, then deletes it.

use std::collections::HashSet;
use std::ffi::CString;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Outbox;
use super::wire::{self, Channel, Report};

/// The most a download may be, received or announced.
pub(crate) const CAP: u64 = 200 * 1000 * 1000;
/// The most downloads running at once; any more are cancelled.
pub(crate) const AT_ONCE: usize = 3;
/// What the disk must keep free while Chrome downloads.
pub(crate) const FREE_FLOOR: u64 = 5 << 30;
/// How often free space is checked while a download runs.
const CHECK_EVERY: Duration = Duration::from_secs(5);
/// The runner's own DevTools ids (its cancels), above factoryd's own
/// (`browser_host::sites::HOST_IDS`, from 2,000,000,000) and within
/// Chrome's 32 bits. Their answers stay here.
const OWN_IDS: u64 = 2_140_000_000;
/// The most of channel 5 waiting to go out before a read waits for it.
const IN_FLIGHT: u64 = 4 << 20;
/// Download events are small: a bigger message isn't one.
const EVENT_MAX: usize = 64 * 1024;

/// The session's downloads folder and the downloads running in it.
pub(crate) struct Downloads {
    dir: PathBuf,
    folder: Arc<OwnedFd>,
    running: HashSet<String>,
    next_id: u64,
    checked: Instant,
}

/// What a message from Chrome meant here.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Seen {
    /// An answer to the runner's own call: not for factoryd.
    pub own: bool,
    /// Downloads to cancel, each with why, in Claktory's words.
    pub cancel: Vec<(String, String)>,
}

impl Downloads {
    /// Makes `<tmp>/downloads` (0700) and holds it open.
    pub(crate) fn new(tmp: &Path) -> std::io::Result<Downloads> {
        use std::os::unix::fs::DirBuilderExt;
        let dir = tmp.join("downloads");
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let path = CString::new(dir.as_os_str().as_encoded_bytes()).map_err(std::io::Error::other)?;
        // SAFETY: open on a NUL-terminated path; the descriptor is owned at once.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: a descriptor just opened, owned only here.
        let folder = Arc::new(unsafe { OwnedFd::from_raw_fd(fd) });
        Ok(Downloads { dir, folder, running: HashSet::new(), next_id: OWN_IDS, checked: Instant::now() })
    }

    /// The folder, as Ready gives it and Chrome writes to it.
    pub(crate) fn dir(&self) -> String {
        self.dir.display().to_string()
    }

    /// Deletes every file in it, finished or not (at every change of mode,
    /// Fugo 36912): what an errand downloaded is never the person's turn's,
    /// and the reverse.
    pub(crate) fn empty(&mut self) {
        self.running.clear();
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                self.unlink(name);
            }
        }
    }

    /// Deletes download `guid` (and Chrome's partial file of it).
    pub(crate) fn delete(&mut self, guid: &str) {
        if wire::guid_ok(guid) {
            self.running.remove(guid);
            self.unlink(guid);
            self.unlink(&format!("{guid}.crdownload"));
        }
    }

    fn unlink(&self, name: &str) {
        let Ok(name) = CString::new(name) else { return };
        // SAFETY: unlinkat relative to the folder's descriptor, a plain name.
        unsafe { libc::unlinkat(self.folder.as_raw_fd(), name.as_ptr(), 0) };
    }

    /// A DevTools message from Chrome, on its way to factoryd: download
    /// events are watched for the caps; answers to the runner's own
    /// cancels stay here.
    pub(crate) fn seen(&mut self, message: &[u8]) -> Seen {
        let mut seen = Seen::default();
        if message.len() > EVENT_MAX {
            return seen;
        }
        let Ok(message) = serde_json::from_slice::<Value>(message) else { return seen };
        if message["id"].as_u64().is_some_and(|id| id >= OWN_IDS) {
            seen.own = true;
            return seen;
        }
        let params = &message["params"];
        let Some(guid) = params["guid"].as_str().filter(|guid| wire::guid_ok(guid)) else { return seen };
        match message["method"].as_str() {
            Some("Browser.downloadWillBegin") => {
                if self.running.len() >= AT_ONCE {
                    seen.cancel.push((guid.to_owned(), format!("more than {AT_ONCE} downloads at once")));
                } else if self.free().is_some_and(|free| free < FREE_FLOOR) {
                    seen.cancel.push((guid.to_owned(), "this computer's disk is too full".to_owned()));
                } else {
                    self.running.insert(guid.to_owned());
                }
            }
            Some("Browser.downloadProgress") => {
                let over = |key: &str| params[key].as_f64().is_some_and(|bytes| bytes > CAP as f64);
                match params["state"].as_str() {
                    Some("inProgress") if over("totalBytes") || over("receivedBytes") => {
                        seen.cancel.push((guid.to_owned(), "it's over the 200 MB cap".to_owned()));
                    }
                    Some("inProgress") => {
                        // One that began before the runner watched it (none
                        // should) counts all the same.
                        if !self.running.contains(guid) && self.running.len() >= AT_ONCE {
                            seen.cancel.push((guid.to_owned(), format!("more than {AT_ONCE} downloads at once")));
                        } else {
                            self.running.insert(guid.to_owned());
                        }
                    }
                    Some("completed") => {
                        self.running.remove(guid);
                        if over("receivedBytes") || self.size(guid).is_some_and(|size| size > CAP) {
                            seen.cancel.push((guid.to_owned(), "it's over the 200 MB cap".to_owned()));
                        }
                    }
                    Some("canceled") => self.delete(guid),
                    _ => {}
                }
            }
            _ => {}
        }
        seen
    }

    /// Every few seconds while downloads run: all cancelled when the disk
    /// runs short.
    pub(crate) fn tick(&mut self) -> Vec<(String, String)> {
        if self.running.is_empty() || self.checked.elapsed() < CHECK_EVERY {
            return Vec::new();
        }
        self.checked = Instant::now();
        if self.free().is_some_and(|free| free < FREE_FLOOR) {
            return self.running.iter().map(|guid| (guid.clone(), "this computer's disk is too full".to_owned())).collect();
        }
        Vec::new()
    }

    /// Chrome's call that cancels `guid`, with the runner's own id.
    pub(crate) fn cancel_call(&mut self, guid: &str) -> Vec<u8> {
        self.next_id = if self.next_id >= i32::MAX as u64 { OWN_IDS } else { self.next_id + 1 };
        json!({"id": self.next_id, "method": "Browser.cancelDownload", "params": {"guid": guid}}).to_string().into_bytes()
    }

    /// Free space on the folder's disk, for a normal user.
    fn free(&self) -> Option<u64> {
        // SAFETY: fstatvfs on our own descriptor, into a zeroed struct.
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        (unsafe { libc::fstatvfs(self.folder.as_raw_fd(), &mut stat) } == 0).then(|| stat.f_bavail as u64 * stat.f_frsize as u64)
    }

    fn size(&self, guid: &str) -> Option<u64> {
        open(&self.folder, guid).ok().and_then(|file| file.metadata().ok()).map(|metadata| metadata.len())
    }

    /// Sends finished download `guid` on channel 5, on a thread of its own
    /// (it may be 200 MB): its GUID and a chunk a frame, then the GUID
    /// alone, then DownloadDone; then deletes it. One that isn't there, or
    /// is over the cap, is DownloadCancelled.
    pub(crate) fn read(&mut self, guid: &str, out: &Outbox) {
        let Some(name) = wire::guid_bytes(guid) else {
            out.report(&Report::DownloadCancelled { guid: String::new(), why: "that isn't a download's name".to_owned() });
            return;
        };
        if self.running.contains(guid) {
            out.report(&Report::DownloadCancelled { guid: guid.to_owned(), why: "it hasn't finished".to_owned() });
            return;
        }
        let file = match open(&self.folder, guid) {
            Ok(file) => file,
            Err(why) => {
                out.report(&Report::DownloadCancelled { guid: guid.to_owned(), why });
                return;
            }
        };
        if file.metadata().map(|metadata| metadata.len()).unwrap_or(u64::MAX) > CAP {
            self.delete(guid);
            out.report(&Report::DownloadCancelled { guid: guid.to_owned(), why: "it's over the 200 MB cap".to_owned() });
            return;
        }
        let (folder, guid, out) = (Arc::clone(&self.folder), guid.to_owned(), out.clone());
        std::thread::spawn(move || {
            let report = match send(file, name, &out) {
                Ok((bytes, sha256)) => Report::DownloadDone { guid: guid.clone(), bytes, sha256 },
                Err(error) => Report::DownloadCancelled { guid: guid.clone(), why: format!("it couldn't be read: {error}") },
            };
            out.report(&report);
            if let Ok(name) = CString::new(guid) {
                // SAFETY: unlinkat relative to the folder's descriptor, a plain name.
                unsafe { libc::unlinkat(folder.as_raw_fd(), name.as_ptr(), 0) };
            }
        });
    }
}

/// Opens `guid` in the folder as a regular file of its own, never through
/// a link, symbolic or hard.
fn open(folder: &OwnedFd, guid: &str) -> Result<std::fs::File, String> {
    let name = CString::new(guid).map_err(|_| "that isn't a download's name".to_owned())?;
    // SAFETY: openat relative to the folder's descriptor, a plain name; the
    // descriptor is owned at once.
    let fd = unsafe { libc::openat(folder.as_raw_fd(), name.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        return Err(if error.kind() == std::io::ErrorKind::NotFound { "it isn't there".to_owned() } else { format!("it couldn't be read: {error}") });
    }
    // SAFETY: a descriptor just opened, owned only here.
    let file = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    // A file of its own: a hard link to another (the profile's cookies,
    // say) is refused too (Fugo 38285).
    match file.metadata() {
        Ok(metadata) if metadata.is_file() && std::os::unix::fs::MetadataExt::nlink(&metadata) == 1 => Ok(file),
        Ok(_) => Err("it isn't a file".to_owned()),
        Err(error) => Err(format!("it couldn't be read: {error}")),
    }
}

/// The file on channel 5, a chunk a frame, waiting while more than
/// IN_FLIGHT of it waits to go out; its size and SHA-256.
fn send(mut file: std::fs::File, name: [u8; 16], out: &Outbox) -> std::io::Result<(u64, String)> {
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut chunk = vec![0u8; wire::MAX_FILE_CHUNK];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > CAP {
            return Err(std::io::Error::other("it grew past the 200 MB cap"));
        }
        hash.update(&chunk[..read]);
        while out.files_waiting() > IN_FLIGHT {
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut payload = Vec::with_capacity(16 + read);
        payload.extend_from_slice(&name);
        payload.extend_from_slice(&chunk[..read]);
        out.send(Channel::Files, payload);
    }
    out.send(Channel::Files, name.to_vec());
    Ok((bytes, crate::encoding::hex(&hash.finalize())))
}

/// Channel 5's bytes queued and not yet written, so a read waits for
/// factoryd rather than holding a whole file in memory.
#[derive(Clone, Default)]
pub(crate) struct Waiting(Arc<AtomicU64>);

impl Waiting {
    pub(crate) fn add(&self, bytes: usize) {
        self.0.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub(crate) fn sent(&self, bytes: usize) {
        self.0.fetch_sub(bytes as u64, Ordering::Relaxed);
    }

    pub(crate) fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUID: &str = "3f1c9a2e-0b6d-4e8f-9a1b-2c3d4e5f6a7b";

    fn folder() -> (PathBuf, Downloads) {
        let tmp = std::env::temp_dir().join(format!("claktory-downloads-{}-{}", std::process::id(), rand_suffix()));
        std::fs::create_dir_all(&tmp).unwrap();
        let downloads = Downloads::new(&tmp).unwrap();
        (tmp, downloads)
    }

    fn rand_suffix() -> u64 {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn event(method: &str, params: Value) -> Vec<u8> {
        json!({"method": method, "params": params}).to_string().into_bytes()
    }

    #[test]
    fn the_runner_holds_chromes_downloads_to_the_caps() {
        let (tmp, mut downloads) = folder();
        let guids: Vec<String> = (0..4).map(|at| format!("3f1c9a2e-0b6d-4e8f-9a1b-2c3d4e5f6a7{at}")).collect();
        for guid in &guids[..3] {
            assert_eq!(downloads.seen(&event("Browser.downloadWillBegin", json!({"guid": guid}))), Seen::default());
        }
        // A fourth at once is cancelled.
        let seen = downloads.seen(&event("Browser.downloadWillBegin", json!({"guid": guids[3]})));
        assert_eq!(seen.cancel, vec![(guids[3].clone(), "more than 3 downloads at once".to_owned())]);
        // Past the cap, announced or received.
        let big = downloads.seen(&event("Browser.downloadProgress", json!({"guid": guids[0], "state": "inProgress", "totalBytes": 300e6, "receivedBytes": 0})));
        assert_eq!(big.cancel, vec![(guids[0].clone(), "it's over the 200 MB cap".to_owned())]);
        let grown = downloads.seen(&event("Browser.downloadProgress", json!({"guid": guids[1], "state": "inProgress", "totalBytes": 0, "receivedBytes": 200_000_001})));
        assert_eq!(grown.cancel.len(), 1);
        // Finished, it no longer counts; cancelled, its file goes.
        std::fs::write(downloads.dir.join(&guids[2]), b"done").unwrap();
        assert_eq!(downloads.seen(&event("Browser.downloadProgress", json!({"guid": guids[2], "state": "completed", "receivedBytes": 4}))), Seen::default());
        assert!(!downloads.running.contains(&guids[2]));
        std::fs::write(downloads.dir.join(&guids[0]), b"partial").unwrap();
        downloads.seen(&event("Browser.downloadProgress", json!({"guid": guids[0], "state": "canceled"})));
        assert!(!downloads.dir.join(&guids[0]).exists() && !downloads.running.contains(&guids[0]));
        // A GUID that isn't one is never looked at.
        assert_eq!(downloads.seen(&event("Browser.downloadWillBegin", json!({"guid": "../../etc/passwd"}))), Seen::default());
        // Answers to its own cancels stay here; factoryd's pass.
        let call: Value = serde_json::from_slice(&downloads.cancel_call(GUID)).unwrap();
        assert_eq!((call["method"].as_str(), call["params"]["guid"].as_str()), (Some("Browser.cancelDownload"), Some(GUID)));
        let id = call["id"].as_u64().unwrap();
        assert!(id > OWN_IDS && id <= i32::MAX as u64);
        assert!(downloads.seen(json!({"id": id, "result": {}}).to_string().as_bytes()).own);
        assert!(!downloads.seen(json!({"id": 2_000_000_005u64, "result": {}}).to_string().as_bytes()).own);
        // Emptied, nothing's left.
        downloads.empty();
        assert_eq!(std::fs::read_dir(&downloads.dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn a_download_is_read_from_its_folder_never_through_a_link() {
        let (tmp, downloads) = folder();
        std::fs::write(tmp.join("secret"), b"not a download").unwrap();
        std::os::unix::fs::symlink(tmp.join("secret"), downloads.dir.join(GUID)).unwrap();
        assert!(open(&downloads.folder, GUID).is_err());
        std::fs::remove_file(downloads.dir.join(GUID)).unwrap();
        std::fs::create_dir(downloads.dir.join(GUID)).unwrap();
        assert_eq!(open(&downloads.folder, GUID).err().as_deref(), Some("it isn't a file"));
        std::fs::remove_dir(downloads.dir.join(GUID)).unwrap();
        std::fs::hard_link(tmp.join("secret"), downloads.dir.join(GUID)).unwrap();
        assert_eq!(open(&downloads.folder, GUID).err().as_deref(), Some("it isn't a file"));
        std::fs::remove_file(downloads.dir.join(GUID)).unwrap();
        assert_eq!(open(&downloads.folder, GUID).err().as_deref(), Some("it isn't there"));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn a_finished_download_goes_out_in_chunks_then_its_guid_then_done() {
        let (tmp, mut downloads) = folder();
        let file: Vec<u8> = (0..(wire::MAX_FILE_CHUNK + 10)).map(|at| at as u8).collect();
        std::fs::write(downloads.dir.join(GUID), &file).unwrap();
        let (sender, frames) = std::sync::mpsc::channel();
        let out = Outbox::new(sender);
        downloads.read(GUID, &out);
        let name = wire::guid_bytes(GUID).unwrap();
        let mut got = Vec::new();
        let done = loop {
            let (channel, payload) = frames.recv_timeout(Duration::from_secs(5)).unwrap();
            out.sent(channel, payload.len());
            match channel {
                Channel::Files => {
                    assert_eq!(&payload[..16], &name);
                    got.extend_from_slice(&payload[16..]);
                }
                Channel::Control => break wire::decode::<Report>(&payload).unwrap(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(got, file);
        assert_eq!(done, Report::DownloadDone { guid: GUID.to_owned(), bytes: file.len() as u64, sha256: crate::encoding::hex(&Sha256::digest(&file)) });
        // Sent, it's gone.
        let deadline = Instant::now() + Duration::from_secs(5);
        while downloads.dir.join(GUID).exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!downloads.dir.join(GUID).exists());
        // Not there: said.
        downloads.read(GUID, &out);
        let (_, payload) = frames.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(wire::decode::<Report>(&payload), Some(Report::DownloadCancelled { guid: GUID.to_owned(), why: "it isn't there".to_owned() }));
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
