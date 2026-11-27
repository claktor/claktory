//! Uploads, the runner's half: a file an agent gives a page (a picture
//! for a post, a CV) is sent by factoryd on channel 5, written to
//! `<the session's folder>/uploads/<id>/<name>`, and only a path staged so
//! may go to Chrome's `DOM.setFileInputFiles` (`lease::judge`): nothing
//! else on this computer reaches a page that way. At most [`CAP`] bytes a
//! file and [`AT_ONCE`] arriving at once; the last [`KEPT`] staged stay
//! (a form may be sent a while after its file was chosen), and every one
//! goes at a change of mode, like downloads.

use std::collections::{HashMap, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::wire::{self, Report};

use super::wire::{MAX_STAGED as KEPT, MAX_UPLOAD as CAP};

/// The most uploads arriving at once.
pub(crate) const AT_ONCE: usize = 2;
/// What the disk keeps free past an upload.
const FREE_FLOOR: u64 = 1 << 30;

/// The session's uploads folder, the files arriving in it and those staged.
pub(crate) struct Uploads {
    dir: PathBuf,
    arriving: HashMap<u64, Arriving>,
    staged: VecDeque<PathBuf>,
}

struct Arriving {
    file: std::fs::File,
    path: PathBuf,
    bytes: u64,
    got: u64,
}

impl Uploads {
    /// Makes `<tmp>/uploads` (0700), kept by its canonical path.
    pub(crate) fn new(tmp: &Path) -> std::io::Result<Uploads> {
        use std::os::unix::fs::DirBuilderExt;
        let dir = tmp.join("uploads");
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let dir = std::fs::canonicalize(&dir)?;
        Ok(Uploads { dir, arriving: HashMap::new(), staged: VecDeque::new() })
    }

    /// Upload `id` of `bytes` called `name` starts arriving; Err says why
    /// not, in Claktory's words.
    pub(crate) fn begin(&mut self, id: u64, name: &str, bytes: u64) -> Result<(), String> {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        if !wire::upload_name_ok(name) {
            return Err("that isn't a file's name".to_owned());
        }
        if bytes > CAP {
            return Err("it's over the 100 MB cap".to_owned());
        }
        if self.arriving.contains_key(&id) {
            return Err("that upload has started already".to_owned());
        }
        if self.arriving.len() >= AT_ONCE {
            return Err(format!("more than {AT_ONCE} uploads at once"));
        }
        if free(&self.dir).is_some_and(|free| free < bytes.saturating_add(FREE_FLOOR)) {
            return Err("this computer's disk is too full".to_owned());
        }
        let folder = self.dir.join(format!("{id:016x}"));
        std::fs::DirBuilder::new().mode(0o700).create(&folder).map_err(|error| format!("couldn't make its folder: {error}"))?;
        let path = folder.join(name);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|error| format!("couldn't make the file: {error}"))?;
        self.arriving.insert(id, Arriving { file, path, bytes, got: 0 });
        Ok(())
    }

    /// One frame of channel 5 in: its upload's id (8 bytes) and a chunk,
    /// or the id alone, which ends it. The report to send, once it's over.
    pub(crate) fn chunk(&mut self, payload: &[u8]) -> Option<Report> {
        let (id, data) = payload.split_first_chunk::<8>()?;
        let id = u64::from_be_bytes(*id);
        let arriving = self.arriving.get_mut(&id)?;
        if data.is_empty() {
            let arriving = self.arriving.remove(&id)?;
            if arriving.got != arriving.bytes {
                remove(&arriving.path);
                return Some(Report::UploadFailed { id, why: format!("{} of its {} bytes came", arriving.got, arriving.bytes) });
            }
            if let Err(error) = arriving.file.sync_all() {
                remove(&arriving.path);
                return Some(Report::UploadFailed { id, why: format!("couldn't write it: {error}") });
            }
            let path = arriving.path.display().to_string();
            self.staged.push_back(arriving.path);
            while self.staged.len() > KEPT {
                if let Some(oldest) = self.staged.pop_front() {
                    remove(&oldest);
                }
            }
            return Some(Report::Uploaded { id, path });
        }
        let over = arriving.got.saturating_add(data.len() as u64) > arriving.bytes;
        let written = if over { Err("more came than it was said to be".to_owned()) } else { arriving.file.write_all(data).map_err(|error| format!("couldn't write it: {error}")) };
        match written {
            Ok(()) => {
                arriving.got += data.len() as u64;
                None
            }
            Err(why) => {
                let path = arriving.path.clone();
                self.arriving.remove(&id);
                remove(&path);
                Some(Report::UploadFailed { id, why })
            }
        }
    }

    /// Whether `path` is a file staged here, exactly as Uploaded said it,
    /// and still that file: a link put in its place leads nowhere.
    pub(crate) fn staged(&self, path: &str) -> bool {
        self.staged.iter().any(|staged| staged.as_os_str() == path)
            && std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
            && std::fs::canonicalize(path).is_ok_and(|real| real.as_os_str() == path)
    }

    /// Deletes every upload, staged or arriving (at every change of mode):
    /// what an errand staged never reaches the person's turn.
    pub(crate) fn empty(&mut self) {
        for arriving in std::mem::take(&mut self.arriving).into_values() {
            remove(&arriving.path);
        }
        for staged in std::mem::take(&mut self.staged) {
            remove(&staged);
        }
    }
}

/// Deletes a staged file and its folder.
fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
    if let Some(folder) = path.parent() {
        let _ = std::fs::remove_dir(folder);
    }
}

/// Free space on `dir`'s disk, for a normal user.
fn free(dir: &Path) -> Option<u64> {
    let path = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: statvfs on a NUL-terminated path, into a zeroed struct.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(path.as_ptr(), &mut stat) } == 0).then(|| stat.f_bavail as u64 * stat.f_frsize as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(id: u64, data: &[u8]) -> Vec<u8> {
        let mut frame = id.to_be_bytes().to_vec();
        frame.extend_from_slice(data);
        frame
    }

    struct Tmp(PathBuf);

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn uploads() -> (Tmp, Uploads) {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = Tmp(std::env::temp_dir().join(format!("claktory-uploads-{}-{n}", std::process::id())));
        let _ = std::fs::remove_dir_all(&tmp.0);
        std::fs::create_dir(&tmp.0).unwrap();
        let uploads = Uploads::new(&tmp.0).unwrap();
        (tmp, uploads)
    }

    #[test]
    fn a_file_is_staged_once_all_of_it_came_and_only_then_goes_to_chrome() {
        let (_tmp, mut uploads) = uploads();
        uploads.begin(1, "cv.pdf", 6).unwrap();
        assert_eq!(uploads.chunk(&frame(1, b"abc")), None);
        assert_eq!(uploads.chunk(&frame(1, b"def")), None);
        let Some(Report::Uploaded { id: 1, path }) = uploads.chunk(&frame(1, b"")) else { panic!("not staged") };
        assert!(path.ends_with("/cv.pdf"));
        assert_eq!(std::fs::read(&path).unwrap(), b"abcdef");
        assert!(uploads.staged(&path));
        assert!(!uploads.staged("/etc/passwd"));
        assert!(!uploads.staged(&format!("{path}/..")));

        uploads.empty();
        assert!(!uploads.staged(&path) && !Path::new(&path).exists(), "a change of mode deletes it");
    }

    #[test]
    fn a_link_put_in_a_staged_files_place_is_no_upload() {
        let (tmp, mut uploads) = uploads();
        std::fs::write(tmp.0.join("Cookies"), b"another profile's").unwrap();
        uploads.begin(1, "photo.jpg", 1).unwrap();
        uploads.chunk(&frame(1, b"x"));
        let Some(Report::Uploaded { path, .. }) = uploads.chunk(&frame(1, b"")) else { panic!("not staged") };
        assert!(uploads.staged(&path));
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(tmp.0.join("Cookies"), &path).unwrap();
        assert!(!uploads.staged(&path));
    }

    #[test]
    fn a_file_that_doesnt_come_as_said_is_deleted() {
        let (_tmp, mut uploads) = uploads();
        uploads.begin(1, "short.png", 6).unwrap();
        uploads.chunk(&frame(1, b"abc"));
        assert!(matches!(uploads.chunk(&frame(1, b"")), Some(Report::UploadFailed { id: 1, .. })));
        uploads.begin(2, "long.png", 2).unwrap();
        assert!(matches!(uploads.chunk(&frame(2, b"abc")), Some(Report::UploadFailed { id: 2, .. })));
        assert_eq!(uploads.chunk(&frame(2, b"")), None, "over once it failed");
        assert_eq!(std::fs::read_dir(&uploads.dir).unwrap().count(), 0, "nothing left of either");
        assert_eq!(uploads.chunk(&frame(9, b"x")), None, "an upload never begun");
    }

    #[test]
    fn names_size_and_how_many_at_once_are_held() {
        let (_tmp, mut uploads) = uploads();
        for name in ["", ".", "..", "a/b", "../x", "a\0b"] {
            assert!(uploads.begin(1, name, 1).is_err(), "{name:?}");
        }
        assert!(uploads.begin(1, "big.mov", CAP + 1).is_err());
        uploads.begin(1, "a", 1).unwrap();
        assert!(uploads.begin(1, "b", 1).is_err(), "the same id twice");
        uploads.begin(2, "b", 1).unwrap();
        assert!(uploads.begin(3, "c", 1).is_err(), "more than {AT_ONCE} at once");
    }

    #[test]
    fn only_the_latest_staged_stay() {
        let (_tmp, mut uploads) = uploads();
        let mut paths = Vec::new();
        for id in 0..=KEPT as u64 {
            uploads.begin(id, "p.jpg", 1).unwrap();
            uploads.chunk(&frame(id, b"x"));
            let Some(Report::Uploaded { path, .. }) = uploads.chunk(&frame(id, b"")) else { panic!("not staged") };
            paths.push(path);
        }
        assert!(!uploads.staged(&paths[0]) && !Path::new(&paths[0]).exists());
        assert!(paths[1..].iter().all(|path| uploads.staged(path)));
    }
}
