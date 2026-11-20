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
//! the manifest's SHA-256, and served as it is.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use factory_protocol::v1;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

/// Archives being made, or that failed, by release folder.
#[derive(Clone, Debug)]
enum Making {
    Running,
    Failed(String),
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
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
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
        Ok(Err(reason)) => return Err(format!("the Mac release isn't what its manifest says: {reason}")),
        Err(error) => return Err(error.to_string()),
    }
    let listed = tokio::process::Command::new(TAR).arg("-tzf").arg(partial).output().await;
    let listed = match listed {
        Ok(output) if output.status.success() => output.stdout,
        Ok(output) => return Err(format!("the Mac release doesn't unpack: {}", String::from_utf8_lossy(&output.stderr).trim())),
        Err(error) => return Err(format!("tar couldn't run: {error}")),
    };
    let app_name = String::from_utf8_lossy(&listed)
        .lines()
        .filter_map(|path| path.trim_start_matches("./").split('/').next())
        .find(|top| top.ends_with(".app"))
        .map(str::to_owned)
        .ok_or("the Mac release has no app in it")?;
    Ok(Meta { size: mac.size, sha256: mac.sha256.clone(), app_name })
}

/// Up to `CHUNK` bytes of the archive from `offset`.
fn read_chunk(archive: &Path, offset: u64, size: u64) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(archive)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut data = Vec::new();
    file.take(CHUNK.min(size - offset)).read_to_end(&mut data)?;
    Ok(data)
}

impl Factory {
    /// A chunk of the app of release `version` from `offset`, or `preparing`
    /// while its archive is being made.
    pub(crate) async fn fetch_app(&self, version: &str, offset: u64) -> Result<v1::AppChunk, RequestError> {
        let Some(release) = release_dir(&self.home.releases(), version) else {
            return Err(no_app(version, "this Claktory has no such release"));
        };
        let Some(meta) = ready(&release) else {
            let mut making = lock();
            match making.get(&release).cloned() {
                Some(Making::Running) => {}
                // Said once; asking again tries again.
                Some(Making::Failed(why)) => {
                    making.remove(&release);
                    return Err(no_app(version, why));
                }
                None => {
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
                                tracing::error!(release = %release.display(), %why, "couldn't pack the app");
                                making.insert(release, Making::Failed(why))
                            }
                        };
                    });
                }
            }
            return Ok(v1::AppChunk { version: version.to_owned(), preparing: true, ..Default::default() });
        };
        if offset > meta.size {
            return Err(no_app(version, format!("offset {offset} is past the archive's end ({} bytes)", meta.size)));
        }
        let archive = release.join(ARCHIVE);
        let size = meta.size;
        let data = tokio::task::spawn_blocking(move || read_chunk(&archive, offset, size))
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
