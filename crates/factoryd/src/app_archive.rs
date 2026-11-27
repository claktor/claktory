//! A release's app, packed for apps on other machines (`FetchApp`).
//!
//! An app paired with this Factory from another machine can't be swapped by
//! the backend, so it fetches its new bundle and swaps itself. The bundle
//! travels as a gzipped tar with the `.app` at its top level, the same form
//! as a published Mac release, in chunks over the connection the app already
//! has (pinned TLS and the device's token).
//!
//! The archive is made the first time an app asks, in the background, and
//! kept in the release's folder (`app.tar.gz`, with `app.tar.gz.json` saying
//! its size, SHA-256 and bundle name), so it costs nothing unless a remote
//! app updates. Until it's ready, `FetchApp` answers `preparing`.
//!
//! A Linux release has no app: its archive is the published Mac release of
//! the same version (`channel::APP_SOURCE`), downloaded and checked against
//! the manifest's SHA-256, and served as it is: a .tar.xz, which
//! every Mac app's `tar -xzf` unpacks (macOS's tar reads xz and gzip by
//! their bytes, ignoring -z), v0.1.22's included. Its app's name is read
//! here in Rust, whatever the compression, so a Linux computer needs no xz.
//! The names stay `app.tar.gz` whatever is inside.
//!
//! A make that failed is said to every ask for a while (`backoff`) rather
//! than tried again at each: an app asks every few seconds, and each try
//! could download the whole Mac release again.

use std::collections::HashMap;
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use factory_protocol::v1;
use serde::{Deserialize, Serialize};

use crate::channel::{APP_SOURCE, Tarball, download_checked};
use crate::factory::{Factory, RequestError};

/// The most a chunk holds.
pub(crate) const CHUNK: u64 = 1 << 20;
const ARCHIVE: &str = "app.tar.gz";
const META: &str = "app.tar.gz.json";
const TAR: &str = if cfg!(target_os = "macos") { "/usr/bin/tar" } else { "tar" };

/// What `META` says of the archive beside it.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Meta {
    size: u64,
    sha256: String,
    /// The bundle at the archive's top level, e.g. "Factory.app".
    app_name: String,
}

/// How long a failed make is said before it's tried again, after the
/// first failure; each failure in a row doubles it, up to `BACKOFF_MOST`.
const BACKOFF: Duration = Duration::from_secs(5 * 60);
const BACKOFF_MOST: Duration = Duration::from_secs(6 * 3600);

/// How much of an xz release is read for its app's name.
const HEAD: usize = 4 << 20;

/// Archives being made, or that failed, by release folder.
#[derive(Clone, Debug)]
enum Making {
    Running,
    /// Said to every ask until `until`, then tried again.
    Failed { why: String, until: Instant, failures: u32 },
}

/// How long to wait after `failures` failures in a row (at least one).
fn backoff(failures: u32) -> Duration {
    BACKOFF.saturating_mul(1 << failures.saturating_sub(1).min(16)).min(BACKOFF_MOST)
}

fn making() -> &'static Mutex<HashMap<PathBuf, Making>> {
    static MAKING: OnceLock<Mutex<HashMap<PathBuf, Making>>> = OnceLock::new();
    MAKING.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<PathBuf, Making>> {
    making().lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn no_app(version: &str, why: impl Into<String>) -> RequestError {
    RequestError::NoApp { version: version.to_owned(), why: why.into() }
}

/// The folder of the release `version`: only a complete release's, by its
/// exact name.
fn release_dir(releases: &Path, version: &str) -> Option<PathBuf> {
    let fine = !version.is_empty()
        && !version.starts_with('.')
        && version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !version.ends_with(".partial");
    let dir = releases.join(version);
    (fine && dir.is_dir()).then_some(dir)
}

/// The `.app` bundle at the top of a release's folder.
fn find_app(release: &Path) -> Option<String> {
    let entries = std::fs::read_dir(release).ok()?;
    let mut apps: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| name.ends_with(".app"))
        .collect();
    apps.sort();
    apps.into_iter().next()
}

/// The Mac release a release with no app gets its app from.
fn source(release: &Path) -> Option<Tarball> {
    serde_json::from_slice(&std::fs::read(release.join(APP_SOURCE)).ok()?).ok()
}

fn ready(release: &Path) -> Option<Meta> {
    let archive = release.join(ARCHIVE);
    let meta: Meta = serde_json::from_slice(&std::fs::read(release.join(META)).ok()?).ok()?;
    (std::fs::metadata(&archive).ok()?.len() == meta.size).then_some(meta)
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    crate::encoding::sha256_file(path, 1 << 16)
}

/// Makes `ARCHIVE`, its meta written first and the archive renamed into
/// place last.
async fn make(release: &Path) -> Result<(), String> {
    let partial = release.join(format!("{ARCHIVE}.partial"));
    let made = match (find_app(release), source(release)) {
        (Some(app_name), _) => pack(release, &partial, app_name).await,
        (None, Some(mac)) => download(&partial, &mac).await,
        (None, None) => Err("the release has no app in it".to_owned()),
    };
    let meta = match made {
        Ok(meta) => meta,
        Err(why) => {
            let _ = tokio::fs::remove_file(&partial).await;
            return Err(why);
        }
    };
    let meta = serde_json::to_vec(&meta).map_err(|error| error.to_string())?;
    tokio::fs::write(release.join(META), meta).await.map_err(|error| error.to_string())?;
    tokio::fs::rename(&partial, release.join(ARCHIVE)).await.map_err(|error| error.to_string())?;
    Ok(())
}

/// Packs the release's app.
async fn pack(release: &Path, partial: &Path, app_name: String) -> Result<Meta, String> {
    let mut tar = tokio::process::Command::new(TAR);
    // Like publish.sh: no resource forks or extended attributes, which the
    // bundle's signature doesn't need.
    tar.env("COPYFILE_DISABLE", "1");
    if cfg!(target_os = "macos") {
        tar.arg("--no-xattrs");
    }
    let packed = tar.arg("-czf").arg(partial).arg("-C").arg(release).arg(&app_name).output().await;
    match packed {
        Ok(output) if output.status.success() => {}
        Ok(output) => return Err(format!("tar failed: {}", String::from_utf8_lossy(&output.stderr).trim())),
        Err(error) => return Err(format!("tar couldn't run: {error}")),
    }
    let hashed = partial.to_owned();
    let sha256 = tokio::task::spawn_blocking(move || sha256_of(&hashed))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("couldn't read the archive: {error}"))?;
    let size = tokio::fs::metadata(partial).await.map_err(|error| error.to_string())?.len();
    Ok(Meta { size, sha256, app_name })
}

/// Downloads the Mac release `mac`, whose tarball is the archive as it is.
async fn download(partial: &Path, mac: &Tarball) -> Result<Meta, String> {
    match download_checked(mac, partial).await {
        Ok(Ok(())) => {}
        // Its size or hashes are for the log, never the person's banner
        // .
        Ok(Err(reason)) => {
            tracing::warn!(url = %mac.url, %reason, "the Mac release isn't what its manifest says");
            return Err("the Mac app it downloaded didn't match the release, so it wasn't used".to_owned());
        }
        Err(error) => return Err(error.to_string()),
    }
    let path = partial.to_owned();
    let app_name = tokio::task::spawn_blocking(move || top_app(&path)).await.map_err(|error| error.to_string())??;
    Ok(Meta { size: mac.size, sha256: mac.sha256.clone(), app_name })
}

/// The `.app` at the top of a Mac release's tarball, read in Rust whatever
/// its compression (xz, gzip in older releases, or none), with no tar or xz
/// on the computer.
fn top_app(tarball: &Path) -> Result<String, String> {
    let unpack = |error: &dyn std::fmt::Display| format!("the Mac release doesn't unpack: {error}");
    let mut file = std::io::BufReader::new(std::fs::File::open(tarball).map_err(|error| unpack(&error))?);
    let magic = file.fill_buf().map_err(|error| unpack(&error))?.to_vec();
    let found = if magic.starts_with(&[0xFD, b'7', b'z', b'X', b'Z', 0]) {
        // xz's decoder writes what it unpacked only as its window fills (up
        // to the whole tar): the start of it is enough.
        let mut head = Head(Vec::new());
        if let Err(error) = lzma_rs::xz_decompress(&mut file, &mut head)
            && head.0.len() < HEAD
        {
            return Err(unpack(&error));
        }
        first_app(tar::Archive::new(head.0.as_slice()))
    } else if magic.starts_with(&[0x1F, 0x8B]) {
        first_app(tar::Archive::new(flate2::read::GzDecoder::new(file)))
    } else {
        first_app(tar::Archive::new(file))
    };
    found.map_err(|error| unpack(&error))?.ok_or_else(|| "the Mac release has no app in it".to_owned())
}

/// The first `.app` at the archive's top level, reading only as far as it.
/// An archive cut short after it (`Head`) still finds it.
fn first_app<R: Read>(mut archive: tar::Archive<R>) -> std::io::Result<Option<String>> {
    for entry in archive.entries()? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => break,
        };
        let path = entry.path()?.into_owned();
        let top = path.components().find_map(|part| match part {
            std::path::Component::Normal(name) => name.to_str().map(str::to_owned),
            _ => None,
        });
        // "._Factory.app" is macOS's AppleDouble of the bundle's metadata,
        // which its tar hides.
        if let Some(top) = top.filter(|top| top.ends_with(".app") && !top.starts_with("._")) {
            return Ok(Some(top));
        }
    }
    Ok(None)
}

/// The first `HEAD` bytes written to it, then an error, which stops the
/// decoder.
struct Head(Vec<u8>);

impl Write for Head {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let room = HEAD - self.0.len();
        if room == 0 {
            return Err(std::io::Error::other("read enough"));
        }
        let taken = buf.len().min(room);
        self.0.extend_from_slice(&buf[..taken]);
        Ok(taken)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The most a chunk holds when the app asks for at most `max_bytes` (0:
/// no smaller than usual).
fn chunk_size(max_bytes: u64) -> u64 {
    if max_bytes == 0 { CHUNK } else { max_bytes.min(CHUNK) }
}

/// Up to `most` bytes of the archive from `offset`.
fn read_chunk(archive: &Path, offset: u64, size: u64, most: u64) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(archive)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut data = Vec::new();
    file.take(most.min(size - offset)).read_to_end(&mut data)?;
    Ok(data)
}

impl Factory {
    /// A chunk of the app of release `version` from `offset`, at most
    /// `max_bytes` (0: the usual `CHUNK`), or `preparing` while its archive
    /// is being made.
    pub(crate) async fn fetch_app(&self, version: &str, offset: u64, max_bytes: u64) -> Result<v1::AppChunk, RequestError> {
        let Some(release) = release_dir(&self.home.releases(), version) else {
            return Err(no_app(version, "this Claktory has no such release"));
        };
        let Some(meta) = ready(&release) else {
            let mut making = lock();
            let failures = match making.get(&release).cloned() {
                Some(Making::Running) => None,
                // Said to every ask for a while, then tried again.
                Some(Making::Failed { why, until, failures }) => {
                    if Instant::now() < until {
                        return Err(no_app(version, why));
                    }
                    Some(failures)
                }
                None => Some(0),
            };
            if let Some(failures) = failures {
                if find_app(&release).is_none() && source(&release).is_none() {
                    return Err(no_app(version, "the release has no app in it"));
                }
                making.insert(release.clone(), Making::Running);
                tokio::spawn(async move {
                    let made = make(&release).await;
                    let mut making = lock();
                    match made {
                        Ok(()) => making.remove(&release),
                        Err(why) => {
                            let failures = failures + 1;
                            let wait = backoff(failures);
                            tracing::error!(release = %release.display(), %why, failures, ?wait, "couldn't pack the app");
                            making.insert(release, Making::Failed { why, until: Instant::now() + wait, failures })
                        }
                    };
                });
            }
            return Ok(v1::AppChunk { version: version.to_owned(), preparing: true, ..Default::default() });
        };
        if offset > meta.size {
            return Err(no_app(version, format!("offset {offset} is past the archive's end ({} bytes)", meta.size)));
        }
        let archive = release.join(ARCHIVE);
        let size = meta.size;
        let most = chunk_size(max_bytes);
        let data = tokio::task::spawn_blocking(move || read_chunk(&archive, offset, size, most))
            .await
            .map_err(|error| RequestError::Io(std::io::Error::other(error)))??;
        Ok(v1::AppChunk {
            version: version.to_owned(),
            offset,
            data,
            size: meta.size,
            sha256: meta.sha256,
            app_name: meta.app_name,
            preparing: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_make_waits_longer_each_time_up_to_six_hours() {
        assert_eq!(backoff(1), Duration::from_secs(300));
        assert_eq!(backoff(2), Duration::from_secs(600));
        assert_eq!(backoff(4), Duration::from_secs(2400));
        assert_eq!(backoff(8), BACKOFF_MOST);
        assert_eq!(backoff(u32::MAX), BACKOFF_MOST);
    }

    /// A published Mac release, downloaded: `CLAKTORY_MAC_RELEASE=<file>
    /// cargo test -p factoryd --lib -- --ignored a_published`.
    #[test]
    #[ignore = "needs a published Mac release"]
    fn a_published_mac_releases_app_is_found() {
        let path = std::env::var_os("CLAKTORY_MAC_RELEASE").expect("CLAKTORY_MAC_RELEASE");
        let started = Instant::now();
        let app = top_app(Path::new(&path)).expect("its app");
        eprintln!("{app} in {:?}", started.elapsed());
        assert!(app.ends_with(".app") && !app.starts_with("._"), "{app}");
    }
}
