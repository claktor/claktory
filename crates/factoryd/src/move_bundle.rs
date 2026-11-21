//! The move bundle (#217): a whole Claktory in one encrypted file, written
//! by `factoryd export` (`move_export`) and read by `factoryd import`. The
//! format is docs/MOVE.md's, version 1; this is its only implementation, so
//! the two sides can't drift apart.
//!
//! Layers, outside in: a clear header (how to open the rest), XChaCha20-
//! Poly1305 in the STREAM construction (chunks sealed in order, the last
//! marked, the header as associated data), zstd, then records: a JSON header
//! each, and for files their content and its SHA-256.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::encoding::{hex, unhex};
use crate::vault::crypto;

/// The version of the format this writes, and the only one it reads.
pub const FORMAT: u32 = 1;
const MAGIC: &[u8; 8] = b"CLAKMOVE";
/// Plaintext bytes per sealed chunk.
const CHUNK: usize = 1 << 20;
const TAG: usize = 16;
const NONCE_PREFIX: usize = 19;
/// Argon2id's costs: 64 MiB, 3 passes, one lane (as the vault's passphrase).
const KDF: (u32, u32, u32) = (65_536, 3, 1);
/// The most a header, the clear one or a record's, may be.
const MOST_HEADER: u32 = 1 << 20;
/// The most the manifest may be.
const MOST_MANIFEST: u64 = 64 << 20;

/// Why a bundle couldn't be written or read, said so the person can act.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("This isn't a Claktory bundle.")]
    NotABundle,
    #[error("This bundle was made by a newer Claktory (bundle format {0}). Update Claktory here first.")]
    NewerFormat(u32),
    #[error(
        "The passphrase doesn't open this bundle. Check it and try again; if it's right, the file is damaged: copy it again from the other computer."
    )]
    Passphrase,
    #[error("The bundle is damaged or incomplete: {0}. Copy it again from the other computer.")]
    Damaged(String),
    #[error("{0}")]
    Io(#[from] io::Error),
}

fn damaged(why: impl Into<String>) -> BundleError {
    BundleError::Damaged(why.into())
}

/// The bundle's clear header.
#[derive(Serialize, Deserialize)]
struct Header {
    format: u32,
    kdf: Kdf,
    cipher: String,
    nonce: String,
    chunk: u32,
    compression: String,
}

#[derive(Serialize, Deserialize)]
struct Kdf {
    name: String,
    m: u32,
    t: u32,
    p: u32,
    salt: String,
}

/// What a bundle holds, as docs/MOVE.md describes it. Unknown fields from a
/// newer writer of the same format are ignored; missing ones are empty.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub format: u32,
    pub export_id: String,
    pub created_ms: i64,
    #[serde(rename = "final")]
    pub is_final: bool,
    pub version: String,
    pub epoch: u64,
    pub source: Source,
    pub files: u64,
    pub bytes: u64,
    pub requires: Requires,
    pub folders: Vec<Folder>,
    pub skipped: Vec<Skipped>,
    pub checkouts: Vec<Checkout>,
    pub tasks: Vec<Task>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub machine: String,
    pub os: String,
    pub arch: String,
    pub user: String,
    pub user_home: String,
    pub home: String,
    pub dev: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Requires {
    pub disk_bytes: u64,
    pub memory_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Folder {
    pub path: String,
    /// "git" or "plain".
    pub kind: String,
    pub projects: Vec<String>,
    pub branch: String,
    pub head: String,
    pub remotes: Vec<Remote>,
    pub bundle: Option<String>,
    pub patch: Option<String>,
    pub untracked: Option<String>,
    pub files: Option<String>,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skipped {
    pub path: String,
    pub why: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Checkout {
    pub agent: String,
    pub name: String,
    pub tree: String,
    pub repo: String,
    pub branch: String,
    pub head: String,
    pub patch: Option<String>,
    pub untracked: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub agent: String,
    pub task: String,
    pub command: String,
    pub started_ms: i64,
}

/// One record's header.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Record {
    File { path: String, size: u64, mode: u32, mtime_ms: i64 },
    Dir { path: String, mode: u32 },
    Symlink { path: String, target: String },
    End { files: u64, bytes: u64 },
}

/// A new passphrase: its text, to print once, and its 20 bytes.
pub fn new_passphrase() -> (String, Zeroizing<[u8; 20]>) {
    crypto::new_recovery()
}

/// A passphrase as typed back; None unless it's one.
pub fn parse_passphrase(text: &str) -> Option<Zeroizing<[u8; 20]>> {
    crypto::parse_recovery(text)
}

fn derive(passphrase: &[u8; 20], kdf: &Kdf) -> Result<crypto::Key, BundleError> {
    let salt = unhex(&kdf.salt).ok_or_else(|| damaged("its header is unreadable"))?;
    if kdf.name != "argon2id" {
        return Err(damaged(format!("it needs a key derivation this Claktory doesn't know ({})", kdf.name)));
    }
    // Bounds, so a forged header can't ask for all the memory there is.
    if kdf.m > 1 << 21 || kdf.t > 16 || kdf.p > 16 {
        return Err(damaged("its header asks too much"));
    }
    let params = argon2::Params::new(kdf.m, kdf.t, kdf.p, Some(32)).map_err(|_| damaged("its header is unreadable"))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    argon.hash_password_into(passphrase, &salt, key.as_mut()).map_err(|_| damaged("its header is unreadable"))?;
    Ok(key)
}

fn nonce(prefix: &[u8; NONCE_PREFIX], counter: u32, last: bool) -> XNonce {
    let mut nonce = [0u8; 24];
    nonce[..NONCE_PREFIX].copy_from_slice(prefix);
    nonce[NONCE_PREFIX..23].copy_from_slice(&counter.to_be_bytes());
    nonce[23] = u8::from(last);
    XNonce::from(nonce)
}

/// Seals what's written to it in chunks; `finish` seals the last.
struct Sealer<W: Write> {
    out: W,
    cipher: XChaCha20Poly1305,
    prefix: [u8; NONCE_PREFIX],
    aad: Vec<u8>,
    counter: u32,
    held: Vec<u8>,
}

impl<W: Write> Sealer<W> {
    fn seal(&mut self, last: bool) -> io::Result<()> {
        let take = self.held.len().min(CHUNK);
        let sealed = self
            .cipher
            .encrypt(&nonce(&self.prefix, self.counter, last), Payload { msg: &self.held[..take], aad: &self.aad })
            .map_err(|_| io::Error::other("sealing failed"))?;
        self.out.write_all(&sealed)?;
        self.held.drain(..take);
        self.counter = self.counter.checked_add(1).ok_or_else(|| io::Error::other("the bundle is too big"))?;
        Ok(())
    }

    fn finish(mut self) -> io::Result<W> {
        self.seal(true)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

impl<W: Write> Write for Sealer<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.held.extend_from_slice(bytes);
        // A full chunk is sealed once more follows it, so the last is known.
        while self.held.len() > CHUNK {
            self.seal(false)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Opens a bundle's chunks in order, checking each and that the last is last.
struct Opener<R: Read> {
    input: BufReader<R>,
    cipher: XChaCha20Poly1305,
    prefix: [u8; NONCE_PREFIX],
    aad: Vec<u8>,
    counter: u32,
    plain: Zeroizing<Vec<u8>>,
    at: usize,
    done: bool,
}

impl<R: Read> Opener<R> {
    fn next_chunk(&mut self) -> Result<(), BundleError> {
        let mut sealed = Vec::with_capacity(CHUNK + TAG);
        (&mut self.input).take((CHUNK + TAG) as u64).read_to_end(&mut sealed)?;
        let at_end = io::BufRead::fill_buf(&mut self.input)?.is_empty();
        if sealed.len() < TAG {
            return Err(damaged("it ends too soon"));
        }
        let opened = self.cipher.decrypt(&nonce(&self.prefix, self.counter, at_end), Payload { msg: &sealed, aad: &self.aad });
        let plain = match opened {
            Ok(plain) => plain,
            // The first chunk not opening is almost always the passphrase.
            Err(_) if self.counter == 0 => {
                // Unless it opens as a chunk that isn't the last: then it was cut short.
                let as_middle = self.cipher.decrypt(&nonce(&self.prefix, 0, !at_end), Payload { msg: &sealed, aad: &self.aad });
                return Err(if as_middle.is_ok() { damaged("it ends too soon") } else { BundleError::Passphrase });
            }
            Err(_) if at_end => return Err(damaged("it ends too soon, or was changed")),
            Err(_) => return Err(damaged("part of it was changed")),
        };
        self.plain = Zeroizing::new(plain);
        self.at = 0;
        self.done = at_end;
        self.counter = self.counter.checked_add(1).ok_or_else(|| damaged("it's too big"))?;
        Ok(())
    }
}

impl<R: Read> Read for Opener<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.at == self.plain.len() {
            if self.done {
                return Ok(0);
            }
            self.next_chunk().map_err(io::Error::other)?;
        }
        let n = out.len().min(self.plain.len() - self.at);
        out[..n].copy_from_slice(&self.plain[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

/// The error behind an io::Error that wraps a BundleError, as itself.
fn unwrap_io(error: io::Error) -> BundleError {
    if error.get_ref().is_some_and(|inner| inner.is::<BundleError>()) {
        let inner = error.into_inner().expect("checked");
        return *inner.downcast::<BundleError>().expect("checked");
    }
    // zstd reports damage as an io::Error too; only an Opener's error is ours.
    if error.kind() == io::ErrorKind::Other || error.kind() == io::ErrorKind::InvalidData || error.kind() == io::ErrorKind::UnexpectedEof {
        return damaged(error.to_string());
    }
    BundleError::Io(error)
}

/// Writes a bundle: records in order, then `finish`.
pub struct Writer<W: Write> {
    out: zstd::stream::write::Encoder<'static, Sealer<W>>,
    files: u64,
    bytes: u64,
}

impl<W: Write> Writer<W> {
    /// Starts a bundle sealed with `passphrase`'s key.
    pub fn new(mut out: W, passphrase: &[u8; 20]) -> io::Result<Writer<W>> {
        let salt: [u8; 16] = crypto::random();
        let prefix: [u8; NONCE_PREFIX] = crypto::random();
        let kdf = Kdf { name: "argon2id".into(), m: KDF.0, t: KDF.1, p: KDF.2, salt: hex(&salt) };
        let key = derive(passphrase, &kdf).map_err(io::Error::other)?;
        let header = Header {
            format: FORMAT,
            kdf,
            cipher: "xchacha20poly1305-stream".into(),
            nonce: hex(&prefix),
            chunk: CHUNK as u32,
            compression: "zstd".into(),
        };
        let header = serde_json::to_vec(&header).map_err(io::Error::other)?;
        let mut aad = MAGIC.to_vec();
        aad.extend_from_slice(&(header.len() as u32).to_be_bytes());
        aad.extend_from_slice(&header);
        out.write_all(&aad)?;
        let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).expect("a 32-byte key");
        let sealer = Sealer { out, cipher, prefix, aad, counter: 0, held: Vec::with_capacity(CHUNK + 1) };
        Ok(Writer { out: zstd::stream::write::Encoder::new(sealer, 3)?, files: 0, bytes: 0 })
    }

    fn header(&mut self, record: &Record) -> io::Result<()> {
        let header = serde_json::to_vec(record).map_err(io::Error::other)?;
        self.out.write_all(&(header.len() as u32).to_be_bytes())?;
        self.out.write_all(&header)
    }

    /// A file of exactly `size` bytes read from `content`; fails if it has
    /// fewer.
    pub fn file(&mut self, path: &str, mode: u32, mtime_ms: i64, size: u64, content: impl Read) -> io::Result<()> {
        check_path(path).map_err(io::Error::other)?;
        self.header(&Record::File { path: path.to_owned(), size, mode, mtime_ms })?;
        let mut hasher = Sha256::new();
        let mut content = content.take(size);
        let mut buffer = vec![0u8; 64 * 1024];
        let mut written = 0u64;
        loop {
            let n = content.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            self.out.write_all(&buffer[..n])?;
            written += n as u64;
        }
        if written != size {
            return Err(io::Error::other(format!("{path} got shorter while it was being read")));
        }
        self.out.write_all(&hasher.finalize())?;
        self.files += 1;
        self.bytes += size;
        Ok(())
    }

    /// A file from memory.
    pub fn bytes(&mut self, path: &str, mode: u32, content: &[u8]) -> io::Result<()> {
        self.file(path, mode, crate::transcript::now_ms(), content.len() as u64, content)
    }

    pub fn dir(&mut self, path: &str, mode: u32) -> io::Result<()> {
        check_path(path).map_err(io::Error::other)?;
        self.header(&Record::Dir { path: path.to_owned(), mode })
    }

    pub fn symlink(&mut self, path: &str, target: &str) -> io::Result<()> {
        check_path(path).map_err(io::Error::other)?;
        check_link(path, target).map_err(io::Error::other)?;
        self.header(&Record::Symlink { path: path.to_owned(), target: target.to_owned() })
    }

    /// Ends the bundle; returns the output, every byte written.
    pub fn finish(mut self) -> io::Result<W> {
        let end = Record::End { files: self.files, bytes: self.bytes };
        self.header(&end)?;
        self.out.finish()?.finish()
    }
}

/// A record's path, checked: relative, `/`-separated, nothing that climbs.
fn check_path(path: &str) -> Result<(), BundleError> {
    let fine = !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..");
    if fine { Ok(()) } else { Err(damaged(format!("it holds a path that isn't allowed ({path})"))) }
}

/// A link's target stays in the bundle's tree, from where the link is.
fn check_link(path: &str, target: &str) -> Result<(), BundleError> {
    let refuse = || damaged(format!("it holds a link that leads out of it ({path} → {target})"));
    if target.is_empty() || target.starts_with('/') || target.contains('\0') {
        return Err(refuse());
    }
    let mut depth: Vec<&str> = path.split('/').collect();
    depth.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                depth.pop().ok_or_else(refuse)?;
            }
            part => depth.push(part),
        }
    }
    Ok(())
}

/// Every link, followed through the tree's other links as the file system
/// will, stays inside the tree: `check_link` alone reads each by itself,
/// and a chain (`b -> .`, then `a -> b/../..`) climbs out where it doesn't.
fn check_chains(links: &HashMap<String, String>) -> Result<(), BundleError> {
    /// More links than this in one path is a loop (as the kernel's own limit).
    const MOST_HOPS: usize = 40;
    for (path, target) in links {
        let refuse = || damaged(format!("it holds a link that leads out of it ({path} → {target})"));
        let mut at: Vec<&str> = path.split('/').collect();
        at.pop();
        let mut parts: std::collections::VecDeque<&str> = target.split('/').collect();
        let mut hops = 0;
        while let Some(part) = parts.pop_front() {
            match part {
                "" | "." => {}
                ".." => {
                    at.pop().ok_or_else(refuse)?;
                }
                name => {
                    at.push(name);
                    if let Some(next) = links.get(&at.join("/")) {
                        hops += 1;
                        if hops > MOST_HOPS || next.starts_with('/') {
                            return Err(refuse());
                        }
                        at.pop();
                        for part in next.split('/').rev() {
                            parts.push_front(part);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// What `read` does after a record.
enum Flow {
    Next,
    Stop,
}

/// Reads a bundle's records in order. `visit` gets each header and, for a
/// file, its content (which `read` then checks against its hash).
fn read(
    bundle: &Path,
    passphrase: &str,
    mut visit: impl FnMut(&Record, Option<&mut dyn Read>) -> Result<Flow, BundleError>,
) -> Result<(), BundleError> {
    let passphrase = parse_passphrase(passphrase).ok_or(BundleError::Passphrase)?;
    let mut file = BufReader::new(File::open(bundle)?);
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).map_err(|_| BundleError::NotABundle)?;
    if &magic != MAGIC {
        return Err(BundleError::NotABundle);
    }
    let mut length = [0u8; 4];
    file.read_exact(&mut length).map_err(|_| BundleError::NotABundle)?;
    let length = u32::from_be_bytes(length);
    if length > MOST_HEADER {
        return Err(BundleError::NotABundle);
    }
    let mut header = vec![0u8; length as usize];
    file.read_exact(&mut header).map_err(|_| damaged("it ends too soon"))?;
    let format: serde_json::Value = serde_json::from_slice(&header).map_err(|_| BundleError::NotABundle)?;
    let version = format.get("format").and_then(serde_json::Value::as_u64).ok_or(BundleError::NotABundle)?;
    if version != u64::from(FORMAT) {
        return Err(BundleError::NewerFormat(u32::try_from(version).unwrap_or(u32::MAX)));
    }
    let parsed: Header = serde_json::from_slice(&header).map_err(|_| damaged("its header is unreadable"))?;
    if parsed.cipher != "xchacha20poly1305-stream" || parsed.compression != "zstd" || parsed.chunk as usize != CHUNK {
        return Err(damaged("its header names a way of sealing this Claktory doesn't know"));
    }
    let prefix: [u8; NONCE_PREFIX] =
        unhex(&parsed.nonce).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| damaged("its header is unreadable"))?;
    let key = derive(&passphrase, &parsed.kdf)?;
    let mut aad = MAGIC.to_vec();
    aad.extend_from_slice(&length.to_be_bytes());
    aad.extend_from_slice(&header);
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref()).expect("a 32-byte key");
    let opener = Opener { input: file, cipher, prefix, aad, counter: 0, plain: Zeroizing::new(Vec::new()), at: 0, done: false };
    let mut stream = zstd::stream::read::Decoder::new(opener).map_err(unwrap_io)?;
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut links: HashMap<String, String> = HashMap::new();
    let mut first = true;
    loop {
        let mut length = [0u8; 4];
        stream.read_exact(&mut length).map_err(unwrap_io)?;
        let length = u32::from_be_bytes(length);
        if length > MOST_HEADER {
            return Err(damaged("a record is too big"));
        }
        let mut header = vec![0u8; length as usize];
        stream.read_exact(&mut header).map_err(unwrap_io)?;
        let record: Record = serde_json::from_slice(&header).map_err(|_| damaged("a record is unreadable"))?;
        if first && !matches!(&record, Record::File { path, .. } if path == "manifest.json") {
            return Err(damaged("it doesn't start with its manifest"));
        }
        first = false;
        let flow = match &record {
            Record::End { files: want_files, bytes: want_bytes } => {
                if (*want_files, *want_bytes) != (files, bytes) {
                    return Err(damaged("it holds fewer files than it says"));
                }
                // Nothing may follow the end.
                let mut rest = [0u8; 1];
                if stream.read(&mut rest).map_err(unwrap_io)? != 0 {
                    return Err(damaged("something follows its end"));
                }
                return check_chains(&links);
            }
            Record::File { path, size, .. } => {
                check_path(path)?;
                let mut hashing = Hashing { inner: (&mut stream).take(*size), hasher: Sha256::new() };
                let flow = visit(&record, Some(&mut hashing))?;
                // What the visitor left unread still counts.
                io::copy(&mut hashing, &mut io::sink()).map_err(unwrap_io)?;
                if hashing.inner.limit() != 0 {
                    return Err(damaged("it ends too soon"));
                }
                let got = hashing.hasher.finalize();
                let mut want = [0u8; 32];
                stream.read_exact(&mut want).map_err(unwrap_io)?;
                if got.as_slice() != want {
                    return Err(damaged(format!("{path} doesn't match its checksum")));
                }
                files += 1;
                bytes += size;
                flow
            }
            Record::Dir { path, .. } => {
                check_path(path)?;
                visit(&record, None)?
            }
            Record::Symlink { path, target } => {
                check_path(path)?;
                check_link(path, target)?;
                links.insert(path.clone(), target.clone());
                visit(&record, None)?
            }
        };
        if let Flow::Stop = flow {
            return Ok(());
        }
    }
}

struct Hashing<R: Read> {
    inner: io::Take<R>,
    hasher: Sha256,
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(out)?;
        self.hasher.update(&out[..n]);
        Ok(n)
    }
}

/// The manifest alone, for checks before unpacking (version, disk, memory).
pub fn read_manifest(bundle: &Path, passphrase: &str) -> Result<Manifest, BundleError> {
    let mut manifest = None;
    read(bundle, passphrase, |record, content| {
        if let (Record::File { size, .. }, Some(content)) = (record, content) {
            manifest = Some(parse_manifest(*size, content)?);
        }
        Ok(Flow::Stop)
    })?;
    manifest.ok_or_else(|| damaged("it has no manifest"))
}

fn parse_manifest(size: u64, content: &mut dyn Read) -> Result<Manifest, BundleError> {
    if size > MOST_MANIFEST {
        return Err(damaged("its manifest is too big"));
    }
    let mut text = Vec::with_capacity(size as usize);
    content.read_to_end(&mut text).map_err(unwrap_io)?;
    let manifest: Manifest = serde_json::from_slice(&text).map_err(|_| damaged("its manifest is unreadable"))?;
    if manifest.format != FORMAT {
        return Err(BundleError::NewerFormat(manifest.format));
    }
    Ok(manifest)
}

/// Reads the whole bundle, checking everything, writing nothing: what
/// export does before it tombstones anything.
pub fn check(bundle: &Path, passphrase: &str) -> Result<Manifest, BundleError> {
    let mut manifest = None;
    read(bundle, passphrase, |record, content| {
        if let (Record::File { path, size, .. }, Some(content)) = (record, content)
            && path == "manifest.json"
        {
            manifest = Some(parse_manifest(*size, content)?);
        }
        Ok(Flow::Next)
    })?;
    manifest.ok_or_else(|| damaged("it has no manifest"))
}

/// Unpacks the bundle into `into` (made if missing): every file checked
/// against its hash as it's written. Safe to run again into the same
/// folder: what's there is replaced, and nothing outside it is touched.
pub fn unpack(bundle: &Path, passphrase: &str, into: &Path) -> Result<Manifest, BundleError> {
    std::fs::create_dir_all(into)?;
    let root = into.canonicalize()?;
    let mut manifest = None;
    let mut made: Vec<PathBuf> = Vec::new();
    let read = read(bundle, passphrase, |record, content| {
        match (record, content) {
            (Record::File { path, size, mode, mtime_ms }, Some(content)) => {
                let target = place(&root, path)?;
                if path == "manifest.json" {
                    let mut text = Vec::new();
                    content.read_to_end(&mut text).map_err(unwrap_io)?;
                    manifest = Some(parse_manifest(*size, &mut text.as_slice())?);
                    std::fs::write(&target, &text)?;
                } else {
                    let mut file = BufWriter::new(File::create(&target)?);
                    io::copy(content, &mut file).map_err(unwrap_io)?;
                    let file = file.into_inner().map_err(|error| error.into_error())?;
                    if let Ok(at) = u64::try_from(*mtime_ms) {
                        let _ = file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(at));
                    }
                }
                set_mode(&target, *mode)?;
            }
            (Record::Dir { path, mode }, _) => {
                let target = root.join(path);
                make_dirs(&root, target.parent().unwrap_or(&root))?;
                if std::fs::symlink_metadata(&target).is_ok_and(|meta| !meta.is_dir()) {
                    std::fs::remove_file(&target)?;
                }
                make_dirs(&root, &target)?;
                set_mode(&target, *mode | 0o700)?;
            }
            (Record::Symlink { path, target: link }, _) => {
                let target = place(&root, path)?;
                std::os::unix::fs::symlink(link, &target)?;
                made.push(target);
            }
            _ => {}
        }
        Ok(Flow::Next)
    });
    // A bundle refused (a chain of links that leads out, say) leaves no link.
    if read.is_err() {
        for link in &made {
            let _ = std::fs::remove_file(link);
        }
    }
    read?;
    manifest.ok_or_else(|| damaged("it has no manifest"))
}

/// Where a file or link goes: its folder made, inside `root` for real
/// (links included), and nothing in its place.
fn place(root: &Path, path: &str) -> Result<PathBuf, BundleError> {
    let target = root.join(path);
    make_dirs(root, target.parent().unwrap_or(root))?;
    match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(&target)?,
        Ok(_) => std::fs::remove_file(&target)?,
        Err(_) => {}
    }
    Ok(target)
}

/// Makes `dir` and the folders above it, once the nearest one that's there
/// is inside `root` for real: never a folder outside it, even through a
/// link that was there before.
fn make_dirs(root: &Path, dir: &Path) -> Result<(), BundleError> {
    let there = dir.ancestors().find(|folder| folder.exists()).unwrap_or(root);
    inside(root, there)?;
    std::fs::create_dir_all(dir)?;
    inside(root, dir)
}

/// `path`, links followed, is inside `root`.
fn inside(root: &Path, path: &Path) -> Result<(), BundleError> {
    let real = path.canonicalize()?;
    if real.starts_with(root) && real.components().all(|part| !matches!(part, Component::ParentDir)) {
        Ok(())
    } else {
        Err(damaged(format!("it would write outside the folder ({})", path.display())))
    }
}

fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("move-bundle-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp");
        dir
    }

    /// Bytes zstd can't shrink, the same every time.
    fn noise(len: usize) -> Vec<u8> {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state as u8
            })
            .collect()
    }

    fn manifest() -> Manifest {
        Manifest { format: FORMAT, export_id: "abc".into(), version: "v1".into(), ..Manifest::default() }
    }

    /// A bundle with a manifest, a file of `big` bytes, a folder and a link.
    fn write(dir: &Path, big: usize) -> (PathBuf, String) {
        let (text, key) = new_passphrase();
        let path = dir.join("test.claktory");
        let mut writer = Writer::new(File::create(&path).expect("create"), &key).expect("writer");
        writer.bytes("manifest.json", 0o644, &serde_json::to_vec(&manifest()).expect("json")).expect("manifest");
        let content = noise(big);
        writer.file("home/agents/a/segment-0001.json", 0o600, 1_700_000_000_000, content.len() as u64, &content[..]).expect("file");
        writer.dir("home/empty", 0o755).expect("dir");
        writer.symlink("home/agents/link", "a/segment-0001.json").expect("link");
        writer.finish().expect("finish");
        (path, text)
    }

    #[test]
    fn a_bundle_unpacks_as_it_was_written_and_again_into_the_same_folder() {
        let dir = temp();
        // Over two chunks, so the stream's order and last chunk count.
        let (bundle, passphrase) = write(&dir, 3 * CHUNK + 17);
        assert_eq!(read_manifest(&bundle, &passphrase).expect("manifest"), manifest());
        assert_eq!(check(&bundle, &passphrase.to_lowercase().replace('-', " ")).expect("checks"), manifest());
        let into = dir.join("out");
        for _ in 0..2 {
            unpack(&bundle, &passphrase, &into).expect("unpack");
            let content = std::fs::read(into.join("home/agents/a/segment-0001.json")).expect("read");
            assert_eq!(content.len(), 3 * CHUNK + 17);
            assert!(content == noise(3 * CHUNK + 17));
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(into.join("home/agents/a/segment-0001.json")).expect("meta").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            assert!(into.join("home/empty").is_dir());
            assert_eq!(std::fs::read_link(into.join("home/agents/link")).expect("link"), Path::new("a/segment-0001.json"));
        }
    }

    #[test]
    fn a_wrong_passphrase_a_changed_byte_or_a_cut_off_end_doesnt_open() {
        let dir = temp();
        let (bundle, passphrase) = write(&dir, 2 * CHUNK + 5);
        let (other, _) = new_passphrase();
        assert!(matches!(check(&bundle, &other), Err(BundleError::Passphrase)));
        assert!(matches!(check(&bundle, "not a passphrase"), Err(BundleError::Passphrase)));
        let bytes = std::fs::read(&bundle).expect("read");

        let changed = dir.join("changed.claktory");
        let mut copy = bytes.clone();
        let at = copy.len() - CHUNK / 2;
        copy[at] ^= 1;
        std::fs::write(&changed, &copy).expect("write");
        assert!(matches!(check(&changed, &passphrase), Err(BundleError::Damaged(_))), "{:?}", check(&changed, &passphrase));

        // Cut after a whole chunk: what's left opens, but isn't the last.
        let header = 12 + u32::from_be_bytes(bytes[8..12].try_into().unwrap()) as usize;
        let cut = dir.join("cut.claktory");
        std::fs::write(&cut, &bytes[..header + CHUNK + TAG]).expect("write");
        assert!(matches!(check(&cut, &passphrase), Err(BundleError::Damaged(_))), "{:?}", check(&cut, &passphrase));

        // A changed header: every chunk has it as associated data.
        let mut header_changed = bytes.clone();
        let at = bytes[12..header].iter().position(|b| *b == b'z').expect("zstd in the header") + 12;
        header_changed[at] = b'Z';
        let forged = dir.join("forged.claktory");
        std::fs::write(&forged, &header_changed).expect("write");
        assert!(check(&forged, &passphrase).is_err());

        std::fs::write(dir.join("plain.txt"), b"hello").expect("write");
        assert!(matches!(check(&dir.join("plain.txt"), &passphrase), Err(BundleError::NotABundle)));
    }

    #[test]
    fn paths_and_links_that_climb_out_are_refused() {
        for path in ["/etc/passwd", "../x", "a/../../x", "a//b", "", "./a"] {
            assert!(check_path(path).is_err(), "{path}");
        }
        assert!(check_path("home/agents/a").is_ok());
        assert!(check_link("home/a/link", "../b").is_ok());
        assert!(check_link("home/link", "../../x").is_err());
        assert!(check_link("link", "/etc").is_err());
    }

    #[test]
    fn a_chain_of_links_that_climbs_out_is_refused_and_leaves_no_link() {
        let chains: [&[(&str, &str)]; 3] = [
            // home/b is home, so home/a is the folder above the tree.
            &[("home/b", "."), ("home/a", "b/../..")],
            &[("home/x/c", "."), ("home/x/b", "c/."), ("home/a", "x/b/c/../../..")],
            // A loop never resolves.
            &[("home/p", "q"), ("home/q", "p/..")],
        ];
        for links in chains {
            let dir = temp();
            let (text, key) = new_passphrase();
            let path = dir.join("chain.claktory");
            let mut writer = Writer::new(File::create(&path).expect("create"), &key).expect("writer");
            writer.bytes("manifest.json", 0o644, &serde_json::to_vec(&manifest()).expect("json")).expect("manifest");
            writer.dir("home/x", 0o755).expect("dir");
            for (link, target) in links {
                writer.symlink(link, target).expect("each link alone stays in");
            }
            writer.finish().expect("finish");
            assert!(matches!(check(&path, &text), Err(BundleError::Damaged(_))), "{links:?}");
            let into = dir.join("out");
            assert!(matches!(unpack(&path, &text, &into), Err(BundleError::Damaged(_))), "{links:?}");
            for (link, _) in links {
                assert!(std::fs::symlink_metadata(into.join(link)).is_err(), "{link} was left");
            }
        }
        // Links through links that stay inside are fine.
        let mut fine = HashMap::new();
        fine.insert("home/b".to_owned(), ".".to_owned());
        fine.insert("home/a".to_owned(), "b/agents/../config.json".to_owned());
        fine.insert("home/c".to_owned(), "a".to_owned());
        assert!(check_chains(&fine).is_ok());
    }
}
