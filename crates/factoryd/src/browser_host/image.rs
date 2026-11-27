//! Claktory's browser image on a Mac (#435 W11; Pericolo 48561, Fugo
//! 48567): an asset of each release, an OCI archive compressed with zstd,
//! never pulled from a registry. The signed factoryd carries what it must
//! be: the archive's SHA-256, the image's manifest digest, and the
//! reference `container image load` gives it, a tag of this release's own.
//!
//! Before each run the loaded image is checked: exactly one variant,
//! linux/arm64, with that digest. It runs by its tag, since `container`
//! asks a registry for any `@sha256` reference. A tag with another digest
//! (a load cut short, a store someone changed) is deleted, never run, and
//! the image is loaded again from the archive: the one in this release's
//! folder, else downloaded from the release over https. The archive is
//! hashed before anything reads it, unpacked into a private folder up to a
//! cap, loaded, and its files deleted whatever happens. Offline with no
//! archive, only a matching image runs. So whoever controls the download
//! can stop the browser starting, never change what runs.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The archive's name in a release's folder.
pub(crate) const ARCHIVE: &str = "claktory-browser.oci.tar.zst";

/// What release.sh compiles in for the macOS build (Pericolo 48569).
const BUILT_IMAGE: Option<&str> = option_env!("CLAKTORY_BROWSER_IMAGE");
const BUILT_REF: Option<&str> = option_env!("CLAKTORY_BROWSER_REF");
const BUILT_ARCHIVE_SHA256: Option<&str> = option_env!("CLAKTORY_BROWSER_ARCHIVE_SHA256");
const BUILT_ARCHIVE_URL: Option<&str> = option_env!("CLAKTORY_BROWSER_ARCHIVE_URL");

/// The most an archive may unpack to, as a multiple of its own size, but
/// never less than `UNPACKED_LEAST` nor more than `UNPACKED_MOST`. Only
/// an archive whose hash this build names is unpacked at all; the cap is
/// in case the release itself were bad, so it can't fill the disk.
const UNPACKED_TIMES: u64 = 16;
const UNPACKED_LEAST: u64 = 1 << 30;
const UNPACKED_MOST: u64 = 8 << 30;
/// The most a download may be.
const DOWNLOAD_MOST: u64 = 4 << 30;

/// The image this build runs, as its build says it must be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Built {
    /// What `container image load` names it, and what it runs:
    /// `localhost/claktory-browser:v0.1.25`.
    pub(crate) reference: String,
    /// Its manifest's digest, `sha256:` and 64 lowercase hex characters.
    pub(crate) digest: String,
    /// The archive's SHA-256, lowercase hex.
    pub(crate) archive_sha256: String,
    /// Where the release keeps the archive; None for a build that ships it
    /// only in its release folder (`--dev`, local).
    pub(crate) url: Option<String>,
}

impl Built {
    /// This build's, or in the dev Claktory one named by FACTORY_BROWSER_IMAGE,
    /// FACTORY_BROWSER_REF and FACTORY_BROWSER_ARCHIVE_SHA256 together.
    pub(crate) fn this(dev: bool) -> Result<Built, String> {
        let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        let chosen = match (dev, env("FACTORY_BROWSER_IMAGE")) {
            (true, Some(image)) => (Some(image), env("FACTORY_BROWSER_REF"), env("FACTORY_BROWSER_ARCHIVE_SHA256"), None),
            _ => (
                BUILT_IMAGE.map(str::to_owned),
                BUILT_REF.map(str::to_owned),
                BUILT_ARCHIVE_SHA256.map(str::to_owned),
                BUILT_ARCHIVE_URL.filter(|url| !url.is_empty()).map(str::to_owned),
            ),
        };
        Built::from(chosen)
    }

    fn from(
        (image, reference, archive_sha256, url): (Option<String>, Option<String>, Option<String>, Option<String>),
    ) -> Result<Built, String> {
        let image = image.ok_or("This build of Claktory has no browser image, so its browser can't run on a Mac.")?;
        if !super::mac::pinned(&image) {
            return Err("This build of Claktory names its browser image without a digest, so it won't run it.".to_owned());
        }
        let digest = image.split_once('@').map(|(_, digest)| digest.to_owned()).unwrap_or_default();
        let reference = reference.filter(|reference| tagged(reference));
        let archive_sha256 = archive_sha256
            .filter(|sha256| sha256.len() == 64 && sha256.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        let (Some(reference), Some(archive_sha256)) = (reference, archive_sha256) else {
            return Err("This build of Claktory doesn't say which browser image archive it runs, so it won't run one.".to_owned());
        };
        if url.as_ref().is_some_and(|url| !url.starts_with("https://")) {
            return Err("This build of Claktory names its browser image's download without https, so it won't use it.".to_owned());
        }
        Ok(Built { reference, digest, archive_sha256, url })
    }
}

/// Whether `reference` is `name:tag`, with nothing a command line could
/// take otherwise and no digest.
fn tagged(reference: &str) -> bool {
    let Some((name, tag)) = reference.rsplit_once(':') else { return false };
    let fine = |part: &str, extra: &[u8]| {
        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || extra.contains(&byte))
    };
    !reference.starts_with('-') && !name.contains(':') && fine(name, b"./_-") && fine(tag, b"._-") && tag.len() <= 128
}

/// Where getting the image is.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Prep {
    Idle,
    Downloading {
        have: u64,
        total: u64,
    },
    Loading,
    /// It couldn't, `failures` times in a row; tried again from `again`.
    Failed {
        why: String,
        failures: u32,
        again: Instant,
    },
}

static PREP: Mutex<Prep> = Mutex::new(Prep::Idle);

fn prep() -> std::sync::MutexGuard<'static, Prep> {
    PREP.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// How long after a failure it's tried again: 2 minutes, doubling up to an
/// hour, so a download that keeps failing its hash isn't fetched again
/// every time the setup card is checked.
fn retry_after(failures: u32) -> Duration {
    Duration::from_secs(120).saturating_mul(1 << failures.saturating_sub(1).min(5)).min(Duration::from_secs(3600))
}

/// What the setup card says while the image is being got, or why it
/// couldn't be; None otherwise.
fn preparing() -> Option<String> {
    match &*prep() {
        Prep::Idle => None,
        Prep::Downloading { have, total } if *total > 0 => Some(format!(
            "Claktory's browser is getting ready: downloading it for this version of Claktory ({} of {} MB). This happens once per update.",
            have >> 20,
            total >> 20
        )),
        Prep::Downloading { .. } => Some(
            "Claktory's browser is getting ready: downloading it for this version of Claktory. This happens once per update.".to_owned(),
        ),
        Prep::Loading => Some("Claktory's browser is getting ready: setting it up on this Mac. It takes a minute.".to_owned()),
        Prep::Failed { why, .. } => Some(format!("Claktory's browser couldn't get ready: {why} Claktory tries again in a few minutes.")),
    }
}

/// Ok when the image this build runs is loaded as it must be; otherwise
/// starts getting it (once at a time, and after a failure only once its
/// wait is over) and says how that goes. Runs `container`: never under the
/// Factory's lock.
pub(crate) fn ensure(home: &Path, container: &Path, built: &Built) -> Result<(), String> {
    if matches!(*prep(), Prep::Downloading { .. } | Prep::Loading) {
        return Err(preparing().unwrap_or_default());
    }
    if ready(container, built) {
        *prep() = Prep::Idle;
        return Ok(());
    }
    let mut state = prep();
    // None: not now (being got, or waiting after a failure).
    let failures = match &*state {
        Prep::Idle => Some(0),
        Prep::Failed { failures, again, .. } => (Instant::now() >= *again).then_some(*failures),
        Prep::Downloading { .. } | Prep::Loading => None,
    };
    if let Some(failures) = failures {
        *state = Prep::Loading;
        let (home, container, built) = (home.to_path_buf(), container.to_path_buf(), built.clone());
        // A thread of its own with its own runtime: the setup card is
        // checked from plain threads too.
        let started = std::thread::Builder::new().name("browser-image".into()).spawn(move || {
            let got = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
                .and_then(|runtime| runtime.block_on(get(&home, &container, &built)));
            *prep() = match got {
                Ok(()) => Prep::Idle,
                Err(why) => {
                    tracing::warn!(%why, "Claktory's browser image couldn't be got");
                    let failures = failures + 1;
                    Prep::Failed { why, failures, again: Instant::now() + retry_after(failures) }
                }
            };
        });
        if let Err(error) = started {
            *state = Prep::Failed {
                why: format!("Claktory couldn't start getting it ({error})."),
                failures: failures + 1,
                again: Instant::now() + retry_after(failures + 1),
            };
        }
    }
    drop(state);
    Err(preparing().unwrap_or_default())
}

/// Whether `built`'s reference is loaded with exactly its one variant; one
/// loaded with anything else is deleted (never run).
fn ready(container: &Path, built: &Built) -> bool {
    let said = std::process::Command::new(container)
        .args(["image", "inspect", &built.reference])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Some(said) = said.ok().filter(|said| said.status.success()) else { return false };
    let inspected: serde_json::Value = serde_json::from_slice(&said.stdout).unwrap_or_default();
    match matches(&inspected, &built.digest) {
        Some(true) => true,
        Some(false) => {
            tracing::warn!(reference = %built.reference, "Claktory's browser image isn't the one this version runs; deleting it to load it again");
            delete(container, &built.reference);
            false
        }
        None => false,
    }
}

/// Whether `container image inspect`'s answer is one image with exactly one
/// variant, linux/arm64, of `digest`; None when it names no image.
fn matches(inspected: &serde_json::Value, digest: &str) -> Option<bool> {
    let image = match inspected {
        serde_json::Value::Array(images) if images.len() == 1 => &images[0],
        serde_json::Value::Array(images) if images.is_empty() => return None,
        serde_json::Value::Object(_) => inspected,
        _ => return Some(false),
    };
    let Some(variants) = image.get("variants").and_then(serde_json::Value::as_array) else { return Some(false) };
    let [variant] = variants.as_slice() else { return Some(false) };
    let platform = |key: &str| variant.pointer(&format!("/platform/{key}")).and_then(serde_json::Value::as_str);
    Some(
        platform("os") == Some("linux")
            && platform("architecture") == Some("arm64")
            && variant.get("digest").and_then(serde_json::Value::as_str) == Some(digest),
    )
}

fn delete(container: &Path, reference: &str) {
    let _ = std::process::Command::new(container)
        .args(["image", "delete", reference])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// The private folder the archive is downloaded and unpacked in (0700).
fn folder(home: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    let folder = home.join("cache").join("browser-image");
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&folder)
        .and_then(|()| std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)))
        .map_err(|error| format!("Claktory can't make its folder for it ({error})."))?;
    Ok(folder)
}

/// This release's own archive, if its folder has one: beside this
/// factoryd, or in the home's folder of this version.
fn shipped(home: &Path) -> Option<PathBuf> {
    let beside = crate::update::running_exe().and_then(|exe| Some(exe.parent()?.join(ARCHIVE)));
    let released = home.join("releases").join(crate::update::VERSION).join(ARCHIVE);
    beside.into_iter().chain([released]).find(|path| path.is_file())
}

/// Gets the image and loads it: the archive copied from this release's
/// folder or downloaded, hashed, unpacked and loaded, its files deleted
/// whatever happens.
async fn get(home: &Path, container: &Path, built: &Built) -> Result<(), String> {
    let folder = folder(home)?;
    let archive = folder.join(format!("{}.oci.tar.zst", built.archive_sha256));
    let unpacked = folder.join("image.tar");
    let result = async {
        match shipped(home) {
            // A copy of its own (a clone on APFS), so what's hashed is what's unpacked.
            Some(path) => {
                std::fs::copy(&path, &archive).map(drop).map_err(|error| format!("Claktory can't read its archive ({error})."))?
            }
            None => {
                let url = built.url.as_deref().ok_or("This version of Claktory came without its browser.")?;
                download(url, &archive).await?;
            }
        }
        let (container, built, archive_path, unpacked_path) = (container.to_path_buf(), built.clone(), archive.clone(), unpacked.clone());
        tokio::task::spawn_blocking(move || load(&container, &built, &archive_path, &unpacked_path))
            .await
            .map_err(|error| error.to_string())?
    }
    .await;
    // A download cut short stays, to carry on from (`.partial`).
    let _ = std::fs::remove_file(&unpacked);
    let _ = std::fs::remove_file(&archive);
    result
}

/// Downloads `url` into `to`, carrying on from a partial download.
async fn download(url: &str, to: &Path) -> Result<(), String> {
    use futures_util::StreamExt as _;
    use tokio::io::AsyncWriteExt as _;

    let partial = to.with_extension("zst.partial");
    let mut have = std::fs::metadata(&partial).map_or(0, |metadata| metadata.len());
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .https_only(true)
        .user_agent(format!("Claktory/{}", crate::update::VERSION))
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client.get(url);
    if have > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let response = request.send().await.map_err(|_| "Claktory couldn't reach github.com to download it.".to_owned())?;
    if response.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        let _ = std::fs::remove_file(&partial);
        return Err("Its download had to start over.".to_owned());
    }
    if !response.status().is_success() {
        return Err(format!("github.com answered {} to its download.", response.status()));
    }
    let resumed = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    if !resumed {
        have = 0;
    }
    let total = response.content_length().map_or(0, |left| left + have);
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&partial)
        .await
        .map_err(|error| format!("Claktory can't write its download ({error})."))?;
    let mut told = Instant::now() - Duration::from_secs(1);
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "Its download stopped partway; it carries on from there next time.".to_owned())?;
        have += chunk.len() as u64;
        if have > DOWNLOAD_MOST {
            drop(file);
            let _ = std::fs::remove_file(&partial);
            return Err("Its download was far bigger than it can be.".to_owned());
        }
        file.write_all(&chunk).await.map_err(|error| format!("Claktory can't write its download ({error})."))?;
        if told.elapsed() >= Duration::from_secs(1) {
            told = Instant::now();
            *prep() = Prep::Downloading { have, total };
        }
    }
    file.flush().await.map_err(|error| error.to_string())?;
    file.sync_all().await.map_err(|error| error.to_string())?;
    drop(file);
    if total > 0 && have < total {
        return Err("Its download stopped partway; it carries on from there next time.".to_owned());
    }
    std::fs::rename(&partial, to).map_err(|error| format!("Claktory can't put its download in place ({error})."))?;
    Ok(())
}

/// Checks `archive` against the build, unpacks it into `unpacked` up to the
/// cap, loads it, and checks what's loaded.
fn load(container: &Path, built: &Built, archive: &Path, unpacked: &Path) -> Result<(), String> {
    *prep() = Prep::Loading;
    let sha256 = crate::encoding::sha256_file(archive, 1 << 20).map_err(|error| format!("Claktory can't read its archive ({error})."))?;
    if sha256 != built.archive_sha256 {
        return Err("The archive isn't the one this version of Claktory was built with, so it wasn't used.".to_owned());
    }
    let size = std::fs::metadata(archive).map_or(0, |metadata| metadata.len());
    unpack(archive, unpacked, size.saturating_mul(UNPACKED_TIMES).clamp(UNPACKED_LEAST, UNPACKED_MOST))?;
    let said = std::process::Command::new(container)
        .args(["image", "load", "-i"])
        .arg(unpacked)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("Apple's container tool didn't run ({error})."))?;
    if !said.status.success() {
        return Err("Apple's container tool couldn't load it.".to_owned());
    }
    let printed = String::from_utf8_lossy(&said.stdout);
    let loaded = printed.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default();
    if loaded != built.reference {
        tracing::warn!(%loaded, expected = %built.reference, "the browser archive loaded under another name");
        return Err("The archive loaded under another name than this version's, so it isn't used.".to_owned());
    }
    if !ready(container, built) {
        return Err("What loaded isn't the image this version of Claktory was built with, so it isn't used.".to_owned());
    }
    forget_earlier(container, built);
    Ok(())
}

/// Deletes earlier versions' tags of this build's image once its own is
/// loaded (Pericolo 49287), so each update doesn't keep another: only
/// names of its own repository, `localhost/claktory-browser:v…` (or
/// `-dev`), never anything else in the store (Fugo 49292).
fn forget_earlier(container: &Path, built: &Built) {
    let said = std::process::Command::new(container)
        .args(["image", "list", "--format", "json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Some(said) = said.ok().filter(|said| said.status.success()) else { return };
    let listed: serde_json::Value = serde_json::from_slice(&said.stdout).unwrap_or_default();
    for name in earlier(&listed, &built.reference) {
        tracing::info!(%name, "deleting an earlier version's browser image");
        delete(container, &name);
    }
}

/// The names in `container image list --format json` (one entry per tag,
/// its name at `configuration.name`) that are earlier tags of
/// `reference`'s repository, Claktory's own only.
fn earlier(listed: &serde_json::Value, reference: &str) -> Vec<String> {
    let Some((repository, _)) = reference.rsplit_once(':') else { return Vec::new() };
    if !matches!(repository, "localhost/claktory-browser" | "localhost/claktory-browser-dev") {
        return Vec::new();
    }
    let entries = listed.as_array().map(Vec::as_slice).unwrap_or_default();
    entries
        .iter()
        .filter_map(|entry| entry.pointer("/configuration/name").and_then(serde_json::Value::as_str))
        .filter(|name| {
            *name != reference
                && tagged(name)
                && name.strip_prefix(repository).and_then(|rest| rest.strip_prefix(":v")).is_some_and(|tag| !tag.is_empty())
        })
        .map(str::to_owned)
        .collect()
}

/// Unpacks the zstd `archive` into `to`, refusing past `most` bytes.
fn unpack(archive: &Path, to: &Path, most: u64) -> Result<(), String> {
    let cant = |error: std::io::Error| format!("Claktory couldn't unpack it ({error}).");
    let input = std::fs::File::open(archive).map_err(cant)?;
    let mut decoder = zstd::stream::read::Decoder::new(input).map_err(cant)?.take(most + 1);
    let mut output = std::fs::File::create(to).map_err(cant)?;
    let written = std::io::copy(&mut decoder, &mut output).map_err(cant)?;
    if written > most {
        drop(output);
        let _ = std::fs::remove_file(to);
        return Err("It unpacked to far more than it can be, so it wasn't used.".to_owned());
    }
    output.flush().map_err(cant)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn built(reference: &str) -> Result<Built, String> {
        Built::from((
            Some(format!("claktory-browser@sha256:{}", "a".repeat(64))),
            Some(reference.to_owned()),
            Some("b".repeat(64)),
            Some("https://github.com/claktory/claktory/releases/download/v0.1.25/claktory-browser-v0.1.25-arm64.oci.tar.zst".to_owned()),
        ))
    }

    #[test]
    fn a_build_names_its_image_by_digest_tag_and_archive_or_none_runs() {
        let image = built("localhost/claktory-browser:v0.1.25").expect("built");
        assert_eq!(image.digest, format!("sha256:{}", "a".repeat(64)));
        assert_eq!(image.reference, "localhost/claktory-browser:v0.1.25");
        for reference in [
            "localhost/claktory-browser",
            "localhost/claktory-browser@sha256:aa",
            "--entrypoint=sh:x",
            "localhost/Claktory:v1",
            "a:b:c",
            "x: y",
        ] {
            assert!(built(reference).is_err(), "{reference}");
        }
        let missing = Built::from((Some(format!("claktory-browser@sha256:{}", "a".repeat(64))), None, Some("b".repeat(64)), None));
        assert!(missing.is_err());
        let short_sha =
            Built::from((Some(format!("x@sha256:{}", "a".repeat(64))), Some("localhost/x:1".into()), Some("b".repeat(63)), None));
        assert!(short_sha.is_err());
        let http = Built::from((
            Some(format!("x@sha256:{}", "a".repeat(64))),
            Some("localhost/x:1".into()),
            Some("b".repeat(64)),
            Some("http://example.com/x".into()),
        ));
        assert!(http.is_err());
        assert!(Built::from((Some("x:latest".into()), Some("localhost/x:1".into()), Some("b".repeat(64)), None)).is_err());
    }

    /// Pericolo 48561: `container image inspect` names the archive's
    /// manifest as its one variant's digest, linux/arm64.
    #[test]
    fn only_one_linux_arm64_variant_of_the_digest_matches() {
        let digest = format!("sha256:{}", "c".repeat(64));
        let inspected = |variants: serde_json::Value| {
            serde_json::json!([{
                "id": format!("sha256:{}", "d".repeat(64)),
                "configuration": {"descriptor": {"digest": format!("sha256:{}", "d".repeat(64)), "mediaType": "application/vnd.oci.image.index.v1+json"}},
                "variants": variants,
            }])
        };
        let variant = |digest: &str, os: &str, architecture: &str| serde_json::json!({"digest": digest, "platform": {"os": os, "architecture": architecture, "variant": "v8"}});
        assert_eq!(matches(&inspected(serde_json::json!([variant(&digest, "linux", "arm64")])), &digest), Some(true));
        let other = format!("sha256:{}", "e".repeat(64));
        assert_eq!(matches(&inspected(serde_json::json!([variant(&other, "linux", "arm64")])), &digest), Some(false));
        assert_eq!(matches(&inspected(serde_json::json!([variant(&digest, "linux", "amd64")])), &digest), Some(false));
        let two = serde_json::json!([variant(&digest, "linux", "arm64"), variant(&other, "linux", "amd64")]);
        assert_eq!(matches(&inspected(two), &digest), Some(false), "exactly one variant");
        assert_eq!(matches(&inspected(serde_json::json!([])), &digest), Some(false));
        assert_eq!(matches(&serde_json::json!([]), &digest), None, "not loaded");
        // The index digest `container` makes up isn't the one compiled in.
        let index = format!("sha256:{}", "d".repeat(64));
        assert_eq!(matches(&inspected(serde_json::json!([variant(&digest, "linux", "arm64")])), &index), Some(false));
    }

    #[test]
    fn an_archive_unpacks_only_up_to_its_cap() {
        let dir = std::env::temp_dir().join(format!("factoryd-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let archive = dir.join("a.tar.zst");
        let data = vec![7u8; 1 << 20];
        std::fs::write(&archive, zstd::encode_all(data.as_slice(), 3).unwrap()).unwrap();
        let to = dir.join("a.tar");
        assert!(unpack(&archive, &to, 1 << 20).is_ok());
        assert_eq!(std::fs::read(&to).unwrap(), data);
        assert!(unpack(&archive, &to, (1 << 20) - 1).is_err(), "past the cap");
        assert!(!to.exists(), "and nothing left of it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failure_waits_longer_each_time_up_to_an_hour() {
        assert_eq!(retry_after(1), Duration::from_secs(120));
        assert_eq!(retry_after(2), Duration::from_secs(240));
        assert_eq!(retry_after(5), Duration::from_secs(1920));
        assert_eq!(retry_after(6), Duration::from_secs(3600));
        assert_eq!(retry_after(40), Duration::from_secs(3600));
    }

    /// A stand-in for `container` in `dir`: `image load -i FILE` keeps a
    /// copy of FILE and prints `loads_as`; `image inspect` prints the one
    /// variant `digest` once something is loaded; `image list` lists it, an
    /// earlier version's tag and someone else's image; `image delete` notes
    /// each name and forgets `loads_as`.
    fn stand_in(dir: &Path, loads_as: &str, digest: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let script = dir.join("container");
        let inspected = serde_json::json!([{"variants": [{"digest": digest, "platform": {"os": "linux", "architecture": "arm64"}}]}]);
        let listed = serde_json::json!([
            {"configuration": {"name": loads_as}},
            {"configuration": {"name": "localhost/claktory-browser:v0.0.1-earlier"}},
            {"configuration": {"name": "docker.io/library/alpine:latest"}},
        ]);
        let d = dir.display();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ncase \"$1 $2\" in\n\
                 'image load') cp \"$4\" '{d}/loaded.tar' && echo 'Loaded image' && echo '{loads_as}' ;;\n\
                 'image inspect') [ -f '{d}/loaded.tar' ] && echo '{inspected}' ;;\n\
                 'image list') echo '{listed}' ;;\n\
                 'image delete') echo \"$3\" >> '{d}/deletes' && [ \"$3\" = '{loads_as}' ] && rm -f '{d}/loaded.tar' ;;\n\
                 *) exit 2 ;;\nesac\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    /// A home with this version's archive of `data` in its release folder,
    /// and the build that names it.
    fn shipping(name: &str, data: &[u8]) -> (PathBuf, Built) {
        let home = std::env::temp_dir().join(format!("factoryd-image-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let release = home.join("releases").join(crate::update::VERSION);
        std::fs::create_dir_all(&release).unwrap();
        let archive = release.join(ARCHIVE);
        std::fs::write(&archive, zstd::encode_all(data, 3).unwrap()).unwrap();
        let built = Built {
            reference: "localhost/claktory-browser:v-test".to_owned(),
            digest: format!("sha256:{}", "c".repeat(64)),
            archive_sha256: crate::encoding::sha256_file(&archive, 1 << 16).unwrap(),
            url: None,
        };
        (home, built)
    }

    fn left_in(home: &Path) -> Vec<String> {
        let names = std::fs::read_dir(home.join("cache").join("browser-image")).unwrap();
        names.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect()
    }

    #[tokio::test]
    async fn the_shipped_archive_loads_once_checked_and_leaves_nothing_behind() {
        let data = b"an OCI archive, as tar".repeat(1000);
        let (home, built) = shipping("loads", &data);
        let container = stand_in(&home, &built.reference, &built.digest);
        assert!(!ready(&container, &built), "nothing loaded yet");
        get(&home, &container, &built).await.expect("loaded");
        assert_eq!(std::fs::read(home.join("loaded.tar")).unwrap(), data, "what loaded is the archive unpacked");
        assert!(ready(&container, &built));
        assert!(left_in(&home).is_empty(), "{:?}", left_in(&home));
        let deleted = std::fs::read_to_string(home.join("deletes")).unwrap_or_default();
        assert_eq!(deleted, "localhost/claktory-browser:v0.0.1-earlier\n", "only the earlier version's tag");
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(home.join("cache").join("browser-image")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn an_archive_this_build_doesnt_name_is_never_loaded() {
        let (home, mut built) = shipping("other-archive", b"someone else's image");
        built.archive_sha256 = "f".repeat(64);
        let container = stand_in(&home, &built.reference, &built.digest);
        let why = get(&home, &container, &built).await.expect_err("refused");
        assert!(why.contains("isn't the one this version"), "{why}");
        assert!(!home.join("loaded.tar").exists(), "never loaded");
        assert!(left_in(&home).is_empty(), "its copy deleted: {:?}", left_in(&home));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn an_image_loaded_under_another_name_or_digest_isnt_run() {
        let (home, built) = shipping("other-name", b"an image");
        let container = stand_in(&home, "localhost/claktory-browser:v0.0.1", &built.digest);
        let why = get(&home, &container, &built).await.expect_err("refused");
        assert!(why.contains("another name"), "{why}");

        // Loaded with another digest (a store someone changed): deleted,
        // never run.
        let (home, built) = shipping("other-digest", b"an image");
        let container = stand_in(&home, &built.reference, &format!("sha256:{}", "e".repeat(64)));
        let why = get(&home, &container, &built).await.expect_err("refused");
        assert!(why.contains("isn't the image"), "{why}");
        assert!(!home.join("loaded.tar").exists(), "deleted");
        assert!(std::fs::read_to_string(home.join("deletes")).is_ok_and(|deletes| !deletes.is_empty()));
        assert!(!ready(&container, &built));
        for home in [home, std::env::temp_dir().join(format!("factoryd-image-other-name-{}", std::process::id()))] {
            let _ = std::fs::remove_dir_all(&home);
        }
    }

    /// Pericolo 49287: `container image list --format json` has an entry
    /// per tag, named at `configuration.name`; `id` differs per tag.
    #[test]
    fn only_earlier_tags_of_its_own_repository_are_deleted() {
        let entry = |name: &str| serde_json::json!({"id": "x", "configuration": {"name": name}, "variants": []});
        let listed = serde_json::json!([
            entry("localhost/claktory-browser-dev:v0.1.24-dev.w11test"),
            entry("localhost/claktory-browser-dev:v0.1.24-dev.w11test2"),
            entry("localhost/claktory-browser:v0.1.24"),
            entry("localhost/claktory-browser:v0.1.25"),
            entry("localhost/claktory-browser:latest"),
            entry("localhost/claktory-browser-extra:v1"),
            entry("docker.io/library/alpine:latest"),
            entry("ghcr.io/claktor/claktory-browser:v0.1.20"),
            serde_json::json!({"id": "y"}),
        ]);
        assert_eq!(earlier(&listed, "localhost/claktory-browser:v0.1.25"), ["localhost/claktory-browser:v0.1.24"]);
        assert_eq!(
            earlier(&listed, "localhost/claktory-browser-dev:v0.1.24-dev.w11test2"),
            ["localhost/claktory-browser-dev:v0.1.24-dev.w11test"],
            "the dev Claktory's own only"
        );
        assert!(earlier(&listed, "localhost/something:v1").is_empty(), "never another repository's");
        assert!(earlier(&serde_json::json!({}), "localhost/claktory-browser:v0.1.25").is_empty());
    }
}
