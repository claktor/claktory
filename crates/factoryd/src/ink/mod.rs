//! Ink (#209 phase 3, `docs/ipad-phase3-contract.md`): what the person
//! draws on the iPad, kept so every device shows it and agents get it only
//! when the person sends it.
//!
//! Blobs (§3): the PencilKit drawing, its PNG preview and review items'
//! context images, uploaded with `UploadPurpose::Ink`, are immutable and
//! named by their SHA-256: `<home>/annotations/blobs/<sha256>` (0600). While
//! one comes in it's in `annotations/staging/`, which move bundles leave out.
//! The whole of `annotations/` is out of every agent's file tools
//! (`private`), admins' included; Bash isn't held to it, as same-user shell
//! access is no OS boundary.
//!
//! What the other parts build on: `blob` (a kept blob, by name), `drawing`
//! and `picture` (one within a kind's limits), and `referenced`, which each
//! table that names blobs answers (layers, review items). Only a referenced
//! blob can be read (ReadInkBlob); one nothing refers to is pruned.

pub(crate) mod layers;

use std::path::{Path, PathBuf};

use factory_protocol::v1;

use crate::factory::{Factory, RequestError};

/// A drawing, at most.
pub(crate) const DRAWING_MAX: u64 = 8 * 1024 * 1024;
/// A PNG: a preview, at most. A context image is held to less when a
/// thread names it.
const PNG_MAX: u64 = 4 * 1024 * 1024;
pub(crate) const PREVIEW_MAX: u64 = PNG_MAX;
/// A review item's context image, at most: bytes, and pixels on its long
/// edge (so read_image passes it through unchanged). Both inclusive.
#[allow(dead_code)] // Review items build on it.
pub(crate) const CONTEXT_MAX: u64 = 3 * 1024 * 1024;
#[allow(dead_code)] // Review items build on it.
pub(crate) const CONTEXT_EDGE_MAX: u32 = 2048;
/// The most of a blob one InkBlobData carries.
const READ_MAX: u64 = 1024 * 1024;

pub(crate) const DRAWING_MIME: &str = "application/x-pencilkit";
pub(crate) const PNG_MIME: &str = "image/png";

/// `<home>/annotations`.
pub(crate) fn root(home: &crate::home::Home) -> PathBuf {
    home.root().join("annotations")
}

/// The folders no agent's file tools reach.
pub(crate) fn private(home: &crate::home::Home) -> Vec<PathBuf> {
    vec![root(home)]
}

fn blobs(home: &crate::home::Home) -> PathBuf {
    root(home).join("blobs")
}

/// Where ink uploads come in, made private if it isn't there yet.
pub(crate) fn staging(home: &crate::home::Home) -> std::io::Result<PathBuf> {
    let staging = root(home).join("staging");
    private_dir(&root(home))?;
    private_dir(&staging)?;
    Ok(staging)
}

/// Makes `dir`, 0700.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    match std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
        Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}

/// The most an ink upload of type `mime` may be.
pub(crate) fn limit(mime: &str) -> Result<u64, String> {
    match mime {
        DRAWING_MIME => Ok(DRAWING_MAX),
        PNG_MIME => Ok(PNG_MAX),
        _ => Err(format!("Ink is a drawing ({DRAWING_MIME}) or a PNG ({PNG_MIME}), not \"{mime}\".")),
    }
}

/// "8 MiB".
pub(crate) fn mib(bytes: u64) -> String {
    format!("{} MiB", bytes / (1024 * 1024))
}

/// A blob's name: 64 lowercase hex digits.
pub(crate) fn valid_sha(sha: &str) -> bool {
    sha.len() == 64 && sha.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Where blob `sha` is kept, if the name is one.
pub(crate) fn blob_path(home: &crate::home::Home, sha: &str) -> Option<PathBuf> {
    valid_sha(sha).then(|| blobs(home).join(sha))
}

/// A PNG's width and height, from its header.
pub(crate) fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    (width > 0 && height > 0).then_some((width, height))
}

fn sync_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

/// Keeps a finished upload, `partial` (whose SHA-256 is `sha`, of type
/// `mime`), as its blob: durable on disk before this returns. One already
/// kept stays as it is.
pub(crate) fn keep(home: &crate::home::Home, partial: &Path, sha: &str, mime: &str) -> Result<(), RequestError> {
    let refused = |why: String| {
        let _ = std::fs::remove_file(partial);
        Err(RequestError::Upload(why))
    };
    if !valid_sha(sha) {
        return refused("An ink blob's SHA-256 is 64 hex digits.".to_owned());
    }
    if mime == PNG_MIME {
        let mut head = [0u8; 24];
        let read = std::fs::File::open(partial).and_then(|mut file| std::io::Read::read_exact(&mut file, &mut head));
        if read.is_err() || png_size(&head).is_none() {
            return refused("That isn't a PNG.".to_owned());
        }
    }
    let dir = blobs(home);
    private_dir(&root(home))?;
    private_dir(&dir)?;
    let path = dir.join(sha);
    if path.is_file() {
        let _ = std::fs::remove_file(partial);
        // Fresh again, so pruning (unreferenced and a day old) can't take
        // it before the save that's about to name it (Mista).
        std::fs::File::options().write(true).open(&path)?.set_modified(std::time::SystemTime::now())?;
        return Ok(());
    }
    {
        use std::os::unix::fs::PermissionsExt as _;
        let file = std::fs::OpenOptions::new().append(true).open(partial)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
    }
    std::fs::rename(partial, &path)?;
    sync_dir(&dir)?;
    Ok(())
}

/// A blob as kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Blob {
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    /// Where it is: only Claktory reads it, or copies it out on Send.
    pub(crate) path: PathBuf,
}

/// Blob `sha`, if it's kept.
pub(crate) fn blob(home: &crate::home::Home, sha: &str) -> Option<Blob> {
    let path = blob_path(home, sha)?;
    let metadata = std::fs::metadata(&path).ok().filter(std::fs::Metadata::is_file)?;
    Some(Blob { sha256: sha.to_owned(), bytes: metadata.len(), path })
}

fn missing(sha: &str) -> String {
    format!("Claktory doesn't have the ink {sha}; upload it first.")
}

/// Blob `sha` as a drawing: kept, and within a drawing's size.
pub(crate) fn drawing(home: &crate::home::Home, sha: &str) -> Result<Blob, String> {
    let blob = blob(home, sha).ok_or_else(|| missing(sha))?;
    if blob.bytes > DRAWING_MAX {
        return Err(format!("A drawing is at most {}.", mib(DRAWING_MAX)));
    }
    Ok(blob)
}

/// Blob `sha` as a PNG of at most `max` bytes and, with `edge`, that many
/// pixels on its long edge: it, and its width and height.
pub(crate) fn picture(home: &crate::home::Home, sha: &str, max: u64, edge: Option<u32>) -> Result<(Blob, u32, u32), String> {
    let blob = blob(home, sha).ok_or_else(|| missing(sha))?;
    if blob.bytes > max {
        return Err(format!("That picture is over {}.", mib(max)));
    }
    let mut head = [0u8; 24];
    let read = std::fs::File::open(&blob.path).and_then(|mut file| std::io::Read::read_exact(&mut file, &mut head));
    let Some((width, height)) = read.ok().and_then(|()| png_size(&head)) else { return Err("That picture isn't a PNG.".to_owned()) };
    if let Some(edge) = edge
        && width.max(height) > edge
    {
        return Err(format!("That picture is over {edge} pixels on its long edge."));
    }
    Ok((blob, width, height))
}

/// Whether anything kept refers to blob `sha`: a layer's revision (its
/// drawing or preview) or a review item (its context image). Only those
/// can be read, and the others are pruned. Each table that names blobs
/// adds its own question here. Callers choose how to fail when the records
/// can't be read: reading refuses, pruning keeps.
pub(crate) fn referenced(db: &rusqlite::Connection, sha: &str) -> rusqlite::Result<bool> {
    Ok(layers::refers_to(db, sha)? || crate::annotations::ink::refers_to(db, sha)?)
}

impl Factory {
    /// ReadInkBlob.
    pub(crate) fn read_ink_blob(&self, request: &v1::ReadInkBlob) -> Result<v1::InkBlobData, RequestError> {
        let unknown = || v1::InkBlobData { sha256: request.sha256.clone(), offset: request.offset, error: "unknown".to_owned(), ..Default::default() };
        let Some(path) = blob_path(&self.home, &request.sha256) else { return Ok(unknown()) };
        if !referenced(&self.store.db(), &request.sha256).unwrap_or(false) {
            return Ok(unknown());
        }
        use std::io::{Read as _, Seek as _};
        let Ok(mut file) = std::fs::File::open(&path) else { return Ok(unknown()) };
        let size = file.metadata()?.len();
        let start = request.offset.min(size);
        let mut data = Vec::new();
        file.seek(std::io::SeekFrom::Start(start))?;
        file.take(READ_MAX).read_to_end(&mut data)?;
        Ok(v1::InkBlobData { sha256: request.sha256.clone(), offset: start, size, data, error: String::new() })
    }
}

/// Removes from `home`'s blobs any file whose name isn't a blob's or whose
/// bytes don't hash to its name (a bundle being imported, say), so a name
/// is always what it says: how many.
pub(crate) fn drop_bad_blobs(home: &crate::home::Home) -> std::io::Result<usize> {
    let Ok(entries) = std::fs::read_dir(blobs(home)) else { return Ok(0) };
    let mut dropped = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let fine = valid_sha(&name)
            && entry.file_type()?.is_file()
            && crate::extensions::viewer::whole_sha256(&entry.path())? == name;
        if !fine {
            let path = entry.path();
            if entry.file_type()?.is_dir() { std::fs::remove_dir_all(&path)? } else { std::fs::remove_file(&path)? }
            dropped += 1;
        }
    }
    Ok(dropped)
}

/// Writes `bytes` as blob `sha` (tests).
#[cfg(test)]
pub(crate) fn put(home: &crate::home::Home, sha: &str, bytes: &[u8]) {
    use std::io::Write as _;
    let staging = staging(home).expect("staging");
    let partial = staging.join(format!("{sha}.partial"));
    std::fs::File::create(&partial).and_then(|mut file| file.write_all(bytes)).expect("written");
    keep(home, &partial, sha, DRAWING_MIME).expect("kept");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_pngs_are_checked() {
        assert!(valid_sha(&"a".repeat(64)));
        assert!(!valid_sha(&"A".repeat(64)) && !valid_sha("../x") && !valid_sha(&"a".repeat(63)));
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&2048u32.to_be_bytes());
        png.extend_from_slice(&1536u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((2048, 1536)));
        assert_eq!(png_size(b"GIF89a"), None);
        assert!(limit("text/html").is_err());
        assert_eq!(limit(DRAWING_MIME), Ok(8 * 1024 * 1024));
    }

    #[test]
    fn blobs_are_found_by_name_and_held_to_their_kind() {
        let dir = std::env::temp_dir().join(format!("factory-ink-{}", std::process::id()));
        let home = crate::home::Home::new(dir.clone());
        let mut context = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        context.extend_from_slice(&1536u32.to_be_bytes());
        context.extend_from_slice(&2048u32.to_be_bytes());
        let (context_sha, wide_sha, drawing_sha) = ("c".repeat(64), "d".repeat(64), "e".repeat(64));
        put(&home, &context_sha, &context);
        let mut wide = context.clone();
        wide[16..20].copy_from_slice(&2049u32.to_be_bytes());
        put(&home, &wide_sha, &wide);
        put(&home, &drawing_sha, b"pencilkit");

        let found = blob(&home, &drawing_sha).expect("kept");
        assert_eq!((found.bytes, found.path.clone()), (9, dir.join(format!("annotations/blobs/{drawing_sha}"))));
        assert_eq!(drawing(&home, &drawing_sha), Ok(found));
        assert!(drawing(&home, &"f".repeat(64)).unwrap_err().contains("upload it first"));
        assert!(blob(&home, "../config.json").is_none());
        let (_, width, height) = picture(&home, &context_sha, CONTEXT_MAX, Some(CONTEXT_EDGE_MAX)).expect("a context image");
        assert_eq!((width, height), (1536, 2048), "2048 on the long edge is fine");
        assert!(picture(&home, &wide_sha, CONTEXT_MAX, Some(CONTEXT_EDGE_MAX)).unwrap_err().contains("over 2048 pixels"));
        assert!(picture(&home, &context_sha, 10, None).unwrap_err().contains("over"));
        assert!(picture(&home, &drawing_sha, PREVIEW_MAX, None).unwrap_err().contains("isn't a PNG"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
