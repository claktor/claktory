//! Releases published online, for Factories installed from one rather than
//! built from a checkout.
//!
//! `scripts/publish.sh` uploads each release as a tarball, beside a manifest,
//! `latest.json`, naming the newest. A backend built with the manifest's
//! address (`FACTORY_UPDATE_URL`, stamped in like the version) reads it
//! shortly after it starts and every half hour. When it names a release newer
//! than the newest one here, it downloads it and puts it where
//! `scripts/release.sh` would: `<home>/releases/<version>/`, then
//! `releases/latest`. From there everything is as for a local release (see
//! `update`): the app offers the update and the person decides when.
//! It never installs anything itself.
//!
//! The manifest's own `url` is the Mac release: the app and the backend. A
//! Linux backend takes its architecture's entry under `linux` instead: the
//! backend alone, a static binary, since the app runs on another machine.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::home::Home;
use crate::update::{VERSION, latest, plain};

/// The manifest's address this build was published with; None for builds
/// that weren't (dev builds, `scripts/release.sh` alone), which never check.
pub const UPDATE_URL: Option<&str> = option_env!("FACTORY_UPDATE_URL");

/// The manifest to check: `FACTORY_UPDATE_URL` in the environment, for
/// trying a channel out, or else the one this build was published with.
pub fn url() -> Option<String> {
    match std::env::var("FACTORY_UPDATE_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url.trim().to_owned()),
        _ => UPDATE_URL.filter(|url| !url.is_empty()).map(str::to_owned),
    }
}

/// The first check, a little after the backend starts.
const FIRST_CHECK: Duration = Duration::from_secs(10);
/// Between checks.
const EVERY: Duration = Duration::from_secs(30 * 60);
const MANIFEST_TIMEOUT_SECONDS: &str = "20";
const MANIFEST_MAX_BYTES: &str = "65536";
const DOWNLOAD_TIMEOUT_SECONDS: &str = "1800";
const MAX_RELEASE_BYTES: u64 = 1 << 30;
/// Releases kept, as `scripts/release.sh` keeps them.
const KEEP: usize = 3;
/// macOS always has these; elsewhere they're wherever PATH finds them.
const CURL: &str = if cfg!(target_os = "macos") { "/usr/bin/curl" } else { "curl" };
const TAR: &str = if cfg!(target_os = "macos") { "/usr/bin/tar" } else { "tar" };
/// In a release with no app (a Linux one), where its Mac app is: the Mac
/// tarball of the same release, as the manifest named it, for apps paired
/// from a Mac to update themselves (`crate::app_archive`).
pub(crate) const APP_SOURCE: &str = "app-source.json";

/// `latest.json`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Manifest {
    pub version: String,
    /// The backend in it: older than `version` when the release only changed
    /// the app. Left out, the same as `version`.
    #[serde(default)]
    pub backend: String,
    /// The tarball: `factoryd`, the app bundle and `backend-version`.
    pub url: String,
    /// The tarball's SHA-256, in hex.
    pub sha256: String,
    /// The tarball's size in bytes.
    pub size: u64,
    /// The oldest macOS it runs on, such as "12.0"; empty if any.
    #[serde(default)]
    pub min_macos: String,
    /// The architectures its backend runs on ("arm64", "x86_64"); empty if any.
    #[serde(default)]
    pub archs: Vec<String>,
    /// The Linux releases, by architecture as Rust names it ("x86_64",
    /// "aarch64"): `factoryd` and `backend-version` in a tarball, no app.
    /// Empty when the release has none.
    #[serde(default)]
    pub linux: BTreeMap<String, Tarball>,
}

/// A release's tarball for one platform.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Tarball {
    pub url: String,
    /// In hex.
    pub sha256: String,
    /// In bytes.
    pub size: u64,
}

impl Tarball {
    fn check(&mut self) -> Result<(), &'static str> {
        if !(self.url.starts_with("https://") || self.url.starts_with("file://")) {
            return Err("the release isn't at an https address");
        }
        if self.sha256.len() != 64 || !self.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("sha256 isn't a SHA-256");
        }
        self.sha256.make_ascii_lowercase();
        if self.size == 0 || self.size > MAX_RELEASE_BYTES {
            return Err("the size is out of range");
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("{0}")]
    Fetch(String),
    #[error("the manifest is unusable: {0}")]
    Manifest(String),
    /// The release itself is wrong; it isn't tried again.
    #[error("release {version} is unusable: {reason}")]
    Rejected { version: String, reason: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What a check found.
#[derive(Debug, PartialEq)]
pub enum Checked {
    /// Nothing newer than what's here.
    UpToDate,
    /// The release named is now in `releases/`, and `releases/latest` names it.
    Downloaded(String),
    /// A newer release this machine can't run, or one already rejected.
    Skipped(String),
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Manifest, ChannelError> {
        let mut manifest: Manifest = serde_json::from_slice(bytes).map_err(|error| ChannelError::Manifest(error.to_string()))?;
        if manifest.backend.is_empty() {
            manifest.backend = manifest.version.clone();
        }
        let bad = |reason: &str| Err(ChannelError::Manifest(reason.to_owned()));
        if !plain(&manifest.version) || !plain(&manifest.backend) {
            return bad("the version isn't one");
        }
        let mut mac = Tarball { url: manifest.url.clone(), sha256: manifest.sha256.clone(), size: manifest.size };
        if let Err(reason) = mac.check() {
            return bad(reason);
        }
        manifest.sha256 = mac.sha256;
        for (arch, tarball) in &mut manifest.linux {
            if let Err(reason) = tarball.check() {
                return bad(&format!("Linux {arch}: {reason}"));
            }
        }
        Ok(manifest)
    }

    /// The tarball for this machine; None when the release has none.
    pub fn here(&self) -> Option<Tarball> {
        if cfg!(target_os = "linux") {
            return self.linux.get(std::env::consts::ARCH).cloned();
        }
        Some(Tarball { url: self.url.clone(), sha256: self.sha256.clone(), size: self.size })
    }
}

pub use crate::update::newer;

/// "aarch64" -> "arm64", as `lipo -archs` names them.
fn this_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        arch => arch,
    }
}

/// "12.0" -> (12, 0); "15.7.3" -> (15, 7).
fn macos_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |minor| minor.parse().ok())?;
    Some((major, minor))
}

async fn this_macos() -> Option<(u32, u32)> {
    let output = tokio::process::Command::new("/usr/bin/sw_vers").arg("-productVersion").output().await.ok()?;
    macos_version(&String::from_utf8_lossy(&output.stdout))
}

/// Why this machine can't run the release; None when it can.
async fn unsupported(manifest: &Manifest) -> Option<String> {
    if !cfg!(target_os = "macos") {
        // A Linux backend is static; its entry is its architecture's.
        return None;
    }
    if !manifest.archs.is_empty() && !manifest.archs.iter().any(|arch| arch == this_arch()) {
        return Some(format!("it's built for {}, and this Mac is {}", manifest.archs.join(" and "), this_arch()));
    }
    let needed = macos_version(&manifest.min_macos)?;
    match this_macos().await {
        Some(here) if here < needed => Some(format!("it needs macOS {} or later", manifest.min_macos)),
        _ => None,
    }
}

/// Fetches with curl, which macOS always has (and the online installer
/// needed elsewhere) and which reads file:// too;
/// what it downloads carries no quarantine, so Gatekeeper treats the app as
/// one built here.
async fn curl(url: &str, timeout: &str, max_bytes: &str, output: Option<&Path>) -> Result<Vec<u8>, ChannelError> {
    let mut command = tokio::process::Command::new(CURL);
    command
        .args(["--fail", "--silent", "--show-error", "--location", "--proto", "=https,file", "--proto-redir", "=https"])
        .args(["--max-time", timeout, "--max-filesize", max_bytes])
        .args(["--user-agent", &format!("Factory/{VERSION}")])
        .kill_on_drop(true);
    if let Some(output) = output {
        command.arg("--output").arg(output);
    }
    let result = command.arg("--").arg(url).output().await?;
    if !result.status.success() {
        let error = String::from_utf8_lossy(&result.stderr);
        return Err(ChannelError::Fetch(format!("couldn't download {url}: {}", error.trim())));
    }
    Ok(result.stdout)
}

/// Downloads `expected` to `to`. Ok(Err(reason)) when what came isn't what
/// the manifest says (its size or SHA-256).
pub(crate) async fn download_checked(expected: &Tarball, to: &Path) -> Result<Result<(), String>, ChannelError> {
    let limit = (expected.size + (1 << 20)).to_string();
    curl(&expected.url, DOWNLOAD_TIMEOUT_SECONDS, &limit, Some(to)).await?;
    let size = tokio::fs::metadata(to).await?.len();
    if size != expected.size {
        return Ok(Err(format!("it's {size} bytes, not the {} the manifest says", expected.size)));
    }
    let hash = sha256(to).await?;
    if hash != expected.sha256 {
        return Ok(Err(format!("its SHA-256 is {hash}, not the manifest's {}", expected.sha256)));
    }
    Ok(Ok(()))
}

async fn sha256(path: &Path) -> std::io::Result<String> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::io::Read;
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
    })
    .await
    .map_err(std::io::Error::other)?
}

async fn remove_all(path: &Path) {
    let removed = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.is_dir() => tokio::fs::remove_dir_all(path).await,
        Ok(_) => tokio::fs::remove_file(path).await,
        Err(_) => return,
    };
    if let Err(error) = removed {
        tracing::warn!(%error, path = %path.display(), "could not clean up a download");
    }
}

/// The app bundle in a release folder (Factory.app; Factory Dev.app in a dev
/// Factory's).
async fn find_app(dir: &Path) -> std::io::Result<Option<PathBuf>> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().is_some_and(|extension| extension == "app") && entry.file_type().await?.is_dir() {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

/// A release's channel, as one backend checks it.
pub struct Channel {
    home: Home,
    url: String,
    /// A release found unusable, by version: it isn't downloaded again.
    rejected: Option<String>,
}

impl Channel {
    pub fn new(home: Home, url: impl Into<String>) -> Channel {
        Channel { home, url: url.into(), rejected: None }
    }

    /// Checks now, and downloads a newer release if there is one.
    pub async fn check(&mut self) -> Result<Checked, ChannelError> {
        let manifest = Manifest::parse(&curl(&self.url, MANIFEST_TIMEOUT_SECONDS, MANIFEST_MAX_BYTES, None).await?)?;
        let here = latest(&self.home).await.version;
        // With no release here yet, the one running is what's here.
        let here = if here.is_empty() && VERSION != "dev" { VERSION.to_owned() } else { here };
        if !here.is_empty() && !newer(&manifest.version, &here) {
            return Ok(Checked::UpToDate);
        }
        if self.rejected.as_deref() == Some(manifest.version.as_str()) {
            return Ok(Checked::Skipped(manifest.version));
        }
        let tarball = match (unsupported(&manifest).await, manifest.here()) {
            (None, Some(tarball)) => tarball,
            (reason, _) => {
                let reason = reason.unwrap_or_else(|| format!("it has no build for Linux {}", std::env::consts::ARCH));
                tracing::info!(version = %manifest.version, %reason, "a newer release can't run here");
                self.rejected = Some(manifest.version.clone());
                return Ok(Checked::Skipped(manifest.version));
            }
        };
        match self.download(&manifest, &tarball).await {
            Err(error @ ChannelError::Rejected { .. }) => {
                self.rejected = Some(manifest.version.clone());
                Err(error)
            }
            other => other,
        }
    }

    async fn download(&self, manifest: &Manifest, tarball_here: &Tarball) -> Result<Checked, ChannelError> {
        let releases = self.home.releases();
        tokio::fs::create_dir_all(&releases).await?;
        let version = &manifest.version;
        let release = releases.join(version);
        // Hidden, so neither `latest()` nor release.sh's pruning counts them.
        let tarball = releases.join(format!(".download-{version}.tar.gz"));
        let partial = releases.join(format!(".partial-{version}"));
        remove_all(&tarball).await;
        remove_all(&partial).await;
        let result = self.unpack(manifest, tarball_here, &tarball, &partial).await;
        remove_all(&tarball).await;
        if let Err(error) = result {
            remove_all(&partial).await;
            return Err(error);
        }
        remove_all(&release).await;
        tokio::fs::rename(&partial, &release).await?;
        // Named only once complete; the rename makes the change atomic.
        let named = releases.join("latest.new");
        tokio::fs::write(&named, format!("{version}\n")).await?;
        tokio::fs::rename(&named, releases.join("latest")).await?;
        tracing::info!(%version, backend = %manifest.backend, "downloaded a new release");
        self.prune().await;
        Ok(Checked::Downloaded(version.clone()))
    }

    async fn unpack(&self, manifest: &Manifest, expected: &Tarball, tarball: &Path, partial: &Path) -> Result<(), ChannelError> {
        let rejected = |reason: String| ChannelError::Rejected { version: manifest.version.clone(), reason };
        if let Err(reason) = download_checked(expected, tarball).await? {
            return Err(rejected(reason));
        }
        tokio::fs::create_dir_all(partial).await?;
        // tar keeps the bundle's symlinks and modes, and refuses paths outside the folder.
        let untar = tokio::process::Command::new(TAR).arg("-xzf").arg(tarball).arg("-C").arg(partial).output().await?;
        if !untar.status.success() {
            return Err(rejected(format!("it doesn't unpack: {}", String::from_utf8_lossy(&untar.stderr).trim())));
        }
        if !tokio::fs::metadata(partial.join("factoryd")).await.is_ok_and(|metadata| metadata.is_file()) {
            return Err(rejected("it has no factoryd".into()));
        }
        // A Linux release has no app: the app runs on another machine.
        if cfg!(target_os = "macos") {
            let Some(app) = find_app(partial).await? else { return Err(rejected("it has no app".into())) };
            let verify =
                tokio::process::Command::new("/usr/bin/codesign").args(["--verify", "--deep", "--strict"]).arg(&app).output().await?;
            if !verify.status.success() {
                return Err(rejected(format!("its app's signature doesn't verify: {}", String::from_utf8_lossy(&verify.stderr).trim())));
            }
        } else {
            // Its app is the Mac release's, fetched when a paired app asks.
            let mac = Tarball { url: manifest.url.clone(), sha256: manifest.sha256.clone(), size: manifest.size };
            tokio::fs::write(partial.join(APP_SOURCE), serde_json::to_vec(&mac).map_err(std::io::Error::other)?).await?;
        }
        tokio::fs::write(partial.join("backend-version"), format!("{}\n", manifest.backend)).await?;
        Ok(())
    }

    /// Keeps the newest few releases, and always the one `latest` names.
    async fn prune(&self) {
        let releases = self.home.releases();
        let named = latest(&self.home).await.version;
        let Ok(mut entries) = tokio::fs::read_dir(&releases).await else { return };
        let mut versions = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if plain(&name) && !name.starts_with("latest") && entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
                versions.push(name);
            }
        }
        versions.sort_unstable_by(|a, b| crate::update::compare_versions(b, a));
        for old in versions.into_iter().skip(KEEP).filter(|version| *version != named) {
            remove_all(&releases.join(old)).await;
        }
    }
}

/// Checks the channel shortly after start and every half hour after, while
/// the backend runs. A failed check is tried again at the next one; the
/// person sees nothing until a release is complete.
pub async fn poll(home: Home, url: String) {
    tracing::info!(%url, "checking for releases");
    let mut channel = Channel::new(home, url);
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        match channel.check().await {
            Ok(Checked::Downloaded(version)) => tracing::info!(%version, "release ready to install"),
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "could not check for releases"),
        }
        tokio::time::sleep(EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(json: serde_json::Value) -> Result<Manifest, ChannelError> {
        Manifest::parse(json.to_string().as_bytes())
    }

    fn good() -> serde_json::Value {
        serde_json::json!({
            "version": "2026.11.20-1012",
            "backend": "2026.11.19-1530",
            "url": "https://github.com/someone/factory-releases/releases/download/2026.11.20-1012/factory-2026.11.20-1012-macos.tar.gz",
            "sha256": "AB".repeat(32),
            "size": 24_174_209,
            "min_macos": "12.0",
            "archs": ["arm64"]
        })
    }

    #[test]
    fn a_manifest_names_a_release_and_its_backend() {
        let parsed = manifest(good()).expect("manifest");
        assert_eq!((parsed.version.as_str(), parsed.backend.as_str()), ("2026.11.20-1012", "2026.11.19-1530"));
        assert_eq!(parsed.sha256, "ab".repeat(32));
        let mut same = good();
        same.as_object_mut().expect("object").remove("backend");
        assert_eq!(manifest(same).expect("manifest").backend, "2026.11.20-1012");
    }

    #[test]
    fn manifests_that_could_do_harm_are_refused() {
        for (key, value) in [
            ("version", serde_json::json!("../../bin")),
            ("version", serde_json::json!("")),
            ("backend", serde_json::json!("a/b")),
            ("url", serde_json::json!("http://example.com/factory.tar.gz")),
            ("sha256", serde_json::json!("abc")),
            ("size", serde_json::json!(0)),
            ("size", serde_json::json!(MAX_RELEASE_BYTES + 1)),
        ] {
            let mut bad = good();
            bad[key] = value.clone();
            assert!(manifest(bad).is_err(), "{key}: {value}");
        }
        assert!(Manifest::parse(b"not json").is_err());
    }

    #[test]
    fn linux_releases_are_checked_like_the_mac_one() {
        let mut with = good();
        with["linux"] = serde_json::json!({"x86_64": {"url": "https://example.com/linux.tar.gz", "sha256": "CD".repeat(32), "size": 10}});
        let parsed = manifest(with.clone()).expect("manifest");
        assert_eq!(parsed.linux["x86_64"].sha256, "cd".repeat(32));
        let here = parsed.here();
        if cfg!(target_os = "macos") {
            assert_eq!(here.map(|tarball| tarball.url), Some(parsed.url.clone()));
        }
        with["linux"]["x86_64"]["url"] = serde_json::json!("http://example.com/linux.tar.gz");
        assert!(manifest(with).is_err());
    }

    #[test]
    fn newer_versions_sort_later() {
        assert!(newer("2026.11.20-1012", "2026.11.19-2359"));
        assert!(newer("2026.11.20-1012.30", "2026.11.20-1012"));
        assert!(!newer("2026.11.20-1012", "2026.11.20-1012"));
        assert!(!newer("2026.11.18-0900", "2026.11.20-1012"));
        // From the date stamps to numbered versions, and on.
        assert!(newer("v0.1.0", "2026.11.19-1958"));
        assert!(!newer("2026.11.19-1958", "v0.1.0"));
        assert!(newer("v0.1.1", "v0.1.0"));
        assert!(newer("v0.1.100", "v0.1.99"));
        assert!(newer("v0.2.0", "v0.1.500"));
        assert!(newer("v1.0.0", "v0.9.9"));
        assert!(!newer("v0.1.9", "v0.1.10"));
        // A dev build comes before the release it leads to, after the one before.
        assert!(newer("v0.1.4", "v0.1.4-dev.20261120-1012"));
        assert!(newer("v0.1.4-dev.20261120-1012", "v0.1.3"));
        assert!(newer("v0.1.4-dev.20261120-1013", "v0.1.4-dev.20261120-1012"));
        // An unstamped build is never offered one.
        assert!(!newer("v0.1.9", "dev"));
    }

    /// A backend from before numbered versions compared them as text: v0.1.0
    /// must look newer to it than its own date stamp, so it offers the update.
    #[test]
    fn the_first_numbered_version_looks_newer_to_a_backend_comparing_text() {
        let old_newer = |candidate: &str, than: &str| candidate > than;
        assert!(old_newer("v0.1.0", "2026.11.19-1958"));
        assert!(old_newer("v0.1.0", "2026.11.19-2200"));
        assert!(old_newer("v0.1.0-dev.20261120-1012", "2026.11.20-0900"));
    }

    #[test]
    fn macos_versions_compare_by_major_then_minor() {
        assert_eq!(macos_version("15.7.3"), Some((15, 7)));
        assert_eq!(macos_version("12"), Some((12, 0)));
        assert_eq!(macos_version(""), None);
        assert!(macos_version("11.6").expect("version") < macos_version("12.0").expect("version"));
    }
}
