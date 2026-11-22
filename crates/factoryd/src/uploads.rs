//! Files the person uploads for an agent, to attach to a message: sent in
//! chunks over the app's connection (StartUpload, UploadChunk, FinishUpload)
//! and stored in the agent's own folder, `<home>/agents/<id>/uploads/
//! <yyyy-mm-dd>/<name>`, for the agent to move where it belongs. While it
//! comes in, a file is kept in `uploads/.partial/`; an upload whose connection
//! dropped resumes from what's there as long as the backend runs.
//!
//! The same chunks carry ink blobs (`UploadPurpose::Ink`, `crate::ink`): for
//! no agent, staged in `<home>/annotations/staging/` and kept by their
//! SHA-256, which their FinishUpload must carry. And dictations
//! (`UploadPurpose::Dictation`, `crate::speech::dictation`): for no agent,
//! kept privately only until they're transcribed.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use factory_protocol::v1;
use sha2::{Digest, Sha256};

use crate::factory::{Factory, RequestError};

/// The biggest file that can be uploaded: 2 GB.
pub(crate) const MAX_SIZE: u64 = 2 * 1024 * 1024 * 1024;
/// The biggest chunk.
const MAX_CHUNK: usize = 1024 * 1024;
/// What an upload leaves free on the disk at least.
const SPARE: u64 = 1024 * 1024 * 1024;
/// Partial files untouched this long are abandoned: deleted when the next
/// upload for the agent starts.
const ABANDONED_MS: u64 = 24 * 3_600_000;

#[derive(Default)]
pub(crate) struct Uploads {
    active: Mutex<HashMap<String, Active>>,
}

/// An upload under way.
struct Active {
    /// Empty for ink and dictations.
    agent: String,
    ink: bool,
    dictation: bool,
    name: String,
    size: u64,
    mime: String,
    partial: PathBuf,
    received: u64,
    hash: Sha256,
}

fn refused(why: impl Into<String>) -> RequestError {
    RequestError::Upload(why.into())
}

/// The file's own name, safe to store: its last part, without control
/// characters, at most 200 bytes (keeping its extension).
fn file_name(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let name: String = name.chars().filter(|character| !character.is_control()).collect();
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    if name.len() <= 200 {
        return Some(name.to_owned());
    }
    let path = Path::new(name);
    let extension = path.extension().and_then(|extension| extension.to_str()).filter(|extension| extension.len() <= 20);
    let suffix = extension.map(|extension| format!(".{extension}")).unwrap_or_default();
    let mut stem: String = String::new();
    for character in name.chars() {
        if stem.len() + character.len_utf8() + suffix.len() > 200 {
            break;
        }
        stem.push(character);
    }
    Some(format!("{stem}{suffix}"))
}

/// `name` in `folder`, or "name 2.ext", "name 3.ext"… when it's taken.
fn free_path(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let (stem, extension) = match (path.file_stem().and_then(|stem| stem.to_str()), path.extension().and_then(|ext| ext.to_str())) {
        (Some(stem), Some(extension)) if !stem.is_empty() => (stem.to_owned(), format!(".{extension}")),
        _ => (name.to_owned(), String::new()),
    };
    (2..).map(|n| folder.join(format!("{stem} {n}{extension}"))).find(|path| !path.exists()).expect("a free name")
}

/// Bytes free for this user on the volume holding `path`.
pub(crate) fn available(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the zeroed struct it's given, for a valid C string.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &raw mut stats) } != 0 {
        return None;
    }
    #[allow(clippy::useless_conversion)] // u32 or u64 depending on the platform
    let (available, block) = (u64::from(stats.f_bavail), u64::from(stats.f_frsize));
    Some(available * block)
}

/// "2026-11-20": today, here.
pub(crate) fn today() -> String {
    // SAFETY: time and localtime_r only write the values they're given.
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&raw const now, &raw mut local) };
    format!("{:04}-{:02}-{:02}", local.tm_year + 1900, local.tm_mon + 1, local.tm_mday)
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Deletes partial files nobody has touched for a day.
fn sweep(partials: &Path) {
    let Ok(entries) = std::fs::read_dir(partials) else { return };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age.as_millis() > u128::from(ABANDONED_MS));
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

impl Factory {
    fn uploads_dir(&self, agent: &str) -> PathBuf {
        self.home.agent_dir(agent).join("uploads")
    }

    /// Starts an upload, or resumes the one with this id: how much the
    /// backend has of it.
    pub(crate) fn start_upload(&self, request: v1::StartUpload) -> Result<v1::UploadProgress, RequestError> {
        let id = request.upload_id.trim().to_owned();
        if id.is_empty() {
            return Err(RequestError::Empty("upload_id"));
        }
        let ink = request.purpose() == v1::UploadPurpose::Ink;
        let dictation = request.purpose() == v1::UploadPurpose::Dictation;
        if ink || dictation {
            if !request.agent_id.is_empty() {
                return Err(refused(if ink {
                    "Ink is uploaded for no agent: leave agent_id empty."
                } else {
                    "A dictation is uploaded for no agent: leave agent_id empty."
                }));
            }
        } else if self.lock().config.agent(&request.agent_id).is_none() {
            return Err(RequestError::UnknownAgent(request.agent_id));
        }
        let Some(name) = file_name(&request.name) else { return Err(refused("The file needs a name.")) };
        if request.size > MAX_SIZE {
            return Err(refused(format!("{name} is {}; files can be up to 2 GB.", gb(request.size))));
        }
        if ink {
            let limit = crate::ink::limit(&request.mime).map_err(refused)?;
            if request.size == 0 || request.size > limit {
                return Err(refused(format!("{name} is {} bytes; ink of this kind is 1 byte to {}.", request.size, crate::ink::mib(limit))));
            }
        }
        if dictation {
            crate::speech::dictation::check_upload(&request.mime, request.size).map_err(refused)?;
        }
        let mut active = self.uploads.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(upload) = active.get(&id)
            && upload.agent == request.agent_id
            && upload.ink == ink
            && upload.dictation == dictation
            && upload.mime == request.mime
            && upload.name == name
            && upload.size == request.size
        {
            return Ok(v1::UploadProgress { upload_id: id, received: upload.received });
        }
        if let Some(old) = active.remove(&id) {
            let _ = std::fs::remove_file(&old.partial);
        }
        let partials = if ink {
            crate::ink::staging(&self.home)?
        } else if dictation {
            crate::speech::dictation::staging(&self.home)?
        } else {
            let partials = self.uploads_dir(&request.agent_id).join(".partial");
            std::fs::create_dir_all(&partials)?;
            partials
        };
        sweep(&partials);
        if let Some(free) = available(&partials)
            && free < request.size + SPARE
        {
            return Err(refused(format!(
                "There isn't room for {name} ({}) on the computer Claktory runs on: {} free, and uploads leave 1 GB.",
                gb(request.size),
                gb(free)
            )));
        }
        let file: String =
            id.chars().map(|character| if character.is_ascii_alphanumeric() || character == '-' { character } else { '_' }).collect();
        let partial = partials.join(format!("{file}-{}{}", crate::transcript::now_ms(), if ink || dictation { ".partial" } else { "" }));
        std::fs::File::create(&partial)?;
        let upload = Active {
            agent: request.agent_id,
            ink,
            dictation,
            name,
            size: request.size,
            mime: request.mime,
            partial,
            received: 0,
            hash: Sha256::new(),
        };
        active.insert(id.clone(), upload);
        Ok(v1::UploadProgress { upload_id: id, received: 0 })
    }

    /// Adds a chunk to an upload: how much the backend has of it now.
    pub(crate) fn upload_chunk(&self, request: v1::UploadChunk) -> Result<v1::UploadProgress, RequestError> {
        let mut active = self.uploads.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(upload) = active.get_mut(&request.upload_id) else {
            return Err(refused("That upload isn't under way; start it again."));
        };
        if request.offset != upload.received {
            return Err(refused(format!("The upload goes on from {} bytes, not {}.", upload.received, request.offset)));
        }
        if request.data.len() > MAX_CHUNK {
            return Err(refused("A chunk can be at most 1 MiB."));
        }
        if upload.received + request.data.len() as u64 > upload.size {
            return Err(refused(format!("That's more than {}'s {} bytes.", upload.name, upload.size)));
        }
        let mut file = std::fs::OpenOptions::new().append(true).open(&upload.partial)?;
        file.write_all(&request.data)?;
        upload.hash.update(&request.data);
        upload.received += request.data.len() as u64;
        Ok(v1::UploadProgress { upload_id: request.upload_id, received: upload.received })
    }

    /// Stores a whole upload in the agent's uploads folder, under the day.
    pub(crate) fn finish_upload(&self, request: v1::FinishUpload) -> Result<v1::UploadedFile, RequestError> {
        let upload = {
            let mut active = self.uploads.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(upload) = active.get(&request.upload_id) else {
                return Err(refused("That upload isn't under way; start it again."));
            };
            if upload.received != upload.size {
                return Err(refused(format!("{} isn't all there yet: {} of {} bytes.", upload.name, upload.received, upload.size)));
            }
            active.remove(&request.upload_id).expect("just found")
        };
        let hash: String = upload.hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
        let expected = request.sha256.trim();
        if upload.ink && expected.is_empty() {
            let _ = std::fs::remove_file(&upload.partial);
            return Err(refused("Ink needs its SHA-256 when it's finished; send it again."));
        }
        if !expected.is_empty() && !expected.eq_ignore_ascii_case(&hash) {
            let _ = std::fs::remove_file(&upload.partial);
            return Err(refused(format!("{} arrived damaged (its SHA-256 differs); send it again.", upload.name)));
        }
        if upload.ink {
            // Checked again on what arrived, not only what was declared.
            let fits = crate::ink::limit(&upload.mime).is_ok_and(|limit| upload.received <= limit);
            if !fits {
                let _ = std::fs::remove_file(&upload.partial);
                return Err(refused(format!("{} is too big for ink.", upload.name)));
            }
            crate::ink::keep(&self.home, &upload.partial, &hash, &upload.mime)?;
            tracing::info!(sha = %hash, size = upload.size, "ink blob kept");
            return Ok(v1::UploadedFile { upload_id: request.upload_id, attachment: None, blob_sha256: hash });
        }
        if upload.dictation {
            self.voices.dictations.uploaded(&self.home, &request.upload_id, &upload.partial)?;
            tracing::info!(size = upload.size, "dictation uploaded");
            return Ok(v1::UploadedFile { upload_id: request.upload_id, attachment: None, blob_sha256: String::new() });
        }
        let folder = self.uploads_dir(&upload.agent).join(today());
        std::fs::create_dir_all(&folder)?;
        let path = free_path(&folder, &upload.name);
        std::fs::rename(&upload.partial, &path)?;
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        tracing::info!(agent = %upload.agent, path = %path.display(), size = upload.size, "file uploaded");
        let attachment = v1::Attachment { path: path.display().to_string(), name, size: upload.size, mime: upload.mime };
        Ok(v1::UploadedFile { upload_id: request.upload_id, attachment: Some(attachment), blob_sha256: String::new() })
    }

    /// Abandons an upload, deleting what came of it.
    pub(crate) fn cancel_upload(&self, request: &v1::CancelUpload) {
        let upload = self.uploads.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&request.upload_id);
        if let Some(upload) = upload {
            let _ = std::fs::remove_file(&upload.partial);
        }
    }

    /// The files a message for `agent` carries, as the agent reads them:
    /// each must be a file in its uploads folder.
    pub(crate) fn attachments(&self, agent: &str, attached: &[v1::Attachment]) -> Result<Vec<factory_agent::Attachment>, RequestError> {
        if attached.is_empty() {
            return Ok(Vec::new());
        }
        let folder = self.uploads_dir(agent).canonicalize().map_err(|_| refused("This agent has no uploads."))?;
        attached
            .iter()
            .map(|attachment| {
                // Checked where it really is; named as the upload answered.
                let real = Path::new(&attachment.path)
                    .canonicalize()
                    .ok()
                    .filter(|path| path.starts_with(&folder) && path.is_file())
                    .ok_or_else(|| refused(format!("{} isn't one of this agent's uploads.", attachment.path)))?;
                let size = std::fs::metadata(&real)?.len();
                let name = real.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                Ok(factory_agent::Attachment { path: attachment.path.clone(), name, size, mime: attachment.mime.clone() })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_the_files_own_and_never_overwrite() {
        assert_eq!(file_name("../../etc/passwd").as_deref(), Some("passwd"));
        assert_eq!(file_name("C:\\Users\\me\\photo.png").as_deref(), Some("photo.png"));
        assert_eq!(file_name(" .. "), None);
        assert_eq!(file_name("a\nb.txt").as_deref(), Some("ab.txt"));
        let long = format!("{}.tar.gz", "x".repeat(300));
        let kept = file_name(&long).expect("a name");
        assert!(kept.len() <= 200 && kept.ends_with("x.gz"), "{kept}");

        let dir = std::env::temp_dir().join(format!("factory-uploads-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        assert_eq!(free_path(&dir, "photo.png"), dir.join("photo.png"));
        std::fs::write(dir.join("photo.png"), "1").expect("write");
        assert_eq!(free_path(&dir, "photo.png"), dir.join("photo 2.png"));
        std::fs::write(dir.join("photo 2.png"), "2").expect("write");
        assert_eq!(free_path(&dir, "photo.png"), dir.join("photo 3.png"));
        std::fs::write(dir.join("notes"), "1").expect("write");
        assert_eq!(free_path(&dir, "notes"), dir.join("notes 2"));
        std::fs::write(dir.join(".env"), "1").expect("write");
        assert_eq!(free_path(&dir, ".env"), dir.join(".env 2"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
