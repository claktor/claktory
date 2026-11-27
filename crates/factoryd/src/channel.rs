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
//! It never installs anything itself. A release it downloaded that the
//! manifest no longer offers (pulled: it names an older one again) is
//! deleted, so it isn't offered any more (`Channel::withdraw`).
//!
//! Replays: each manifest is stamped when it's published (`issued_ms`,
//! inside what's signed). A backend keeps the newest stamp it accepted per
//! channel (`ISSUED`) and refuses an older manifest, or an unstamped one
//! once it has seen a stamped one, so an old signed `latest.json` served
//! again can neither roll the offer back nor take back a newer release. Only
//! a stamped manifest newer than the one a release came from withdraws it.
//!
//! The manifest's own `url` is the Mac release: the app and the backend. A
//! Linux backend takes its architecture's entry under `linux` instead: the
//! backend alone, a static binary, since the app runs on another machine.
//!
//! Who may publish: a manifest from an online channel must be signed by one
//! of `RELEASE_KEYS` (`latest.json.sig` beside it, written by
//! `scripts/publish.sh`), and since it names every tarball's SHA-256, that
//! pins everything the release holds. On a Mac, a backend that's signed with
//! a Developer ID also takes only apps, backends and helpers signed by its
//! own team. A channel in a folder (`file://`, for trying releases out, and
//! `--dev` ones) is trusted as it is: whoever can write there can already
//! change this Claktory's files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};

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
const SIGNATURE_MAX_BYTES: &str = "4096";

/// The keys releases are signed with, by key id (the part of
/// `latest.json.sig` before its colon): raw ed25519 public keys, in base64.
/// The working key, and a recovery key kept offline, so a lost or exposed
/// working key can still be replaced by a signed release.
pub const RELEASE_KEYS: &[(&str, &str)] = &[
    // The working key: in the vault, used by scripts/publish.sh.
    ("claktory-release-1", "1XSGB3t02Vb1Jy23RSfzXk+lF+vGynERpSbLzd6ZlbU="),
    // The recovery key: kept offline only, to sign a release that replaces
    // a lost or exposed working key.
    ("claktory-release-recovery-1", "aGdAkbnB7pkB/dRRfPnkFKfKyWzPs+cF7FVHMlb7JpM="),
];

/// The pinned release keys, ready to check with.
pub fn release_keys() -> Vec<(String, VerifyingKey)> {
    RELEASE_KEYS.iter().filter_map(|(id, key)| Some(((*id).to_owned(), public_key(key)?))).collect()
}

/// An ed25519 public key from its base64.
pub fn public_key(base64: &str) -> Option<VerifyingKey> {
    let bytes: [u8; 32] = STANDARD.decode(base64.trim()).ok()?.try_into().ok()?;
    VerifyingKey::from_bytes(&bytes).ok()
}

/// Whether a manifest from `url` must be signed: everything but a channel
/// in a folder (`file://`), which is how dev and local channels are tried.
pub fn requires_signature(url: &str) -> bool {
    !url.trim_start().starts_with("file://")
}

/// Checks `sidecar` (`<key id>:<base64 signature>`) is a signature of the
/// manifest's exact bytes by one of `keys`. Err says why not.
pub fn verify_manifest(manifest: &[u8], sidecar: &[u8], keys: &[(String, VerifyingKey)]) -> Result<(), String> {
    let line = std::str::from_utf8(sidecar).map_err(|_| "its signature isn't text".to_owned())?.trim();
    let (id, signature) = line.split_once(':').ok_or_else(|| "its signature names no key".to_owned())?;
    let shown: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(40).collect();
    let Some((_, key)) = keys.iter().find(|(known, _)| known == id) else {
        return Err(format!("it's signed with a key this Claktory doesn't know ({shown})"));
    };
    let bytes = STANDARD.decode(signature.trim()).map_err(|_| "its signature isn't base64".to_owned())?;
    let signature = Signature::from_slice(&bytes).map_err(|_| "its signature isn't an ed25519 one".to_owned())?;
    key.verify_strict(manifest, &signature).map_err(|_| format!("its signature by {shown} doesn't match it"))
}

/// The team a Developer ID signature names, from `codesign -dv`'s output;
/// None for one signed ad hoc.
fn team_in(codesign_output: &str) -> Option<String> {
    let team = codesign_output.lines().find_map(|line| line.strip_prefix("TeamIdentifier="))?.trim();
    (!team.is_empty() && team.len() <= 20 && team.chars().all(|c| c.is_ascii_alphanumeric())).then(|| team.to_owned())
}

/// The team this backend is signed by, if a Developer ID signed it.
async fn own_team() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let output = tokio::process::Command::new("/usr/bin/codesign").arg("-dv").arg(&exe).output().await.ok()?;
    team_in(&String::from_utf8_lossy(&output.stderr))
}

/// codesign's argument for the requirement that holds only for code signed
/// by `team`'s Developer ID Application certificate (`-R=` then the
/// requirement's text): Apple's chain, the team, and the Developer ID
/// Application field, so not the same team's development certificates.
fn team_requirement(team: &str) -> String {
    format!(
        "-R=anchor apple generic and certificate leaf[subject.OU] = \"{team}\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
    )
}

const DOWNLOAD_TIMEOUT_SECONDS: &str = "1800";
const MAX_RELEASE_BYTES: u64 = 1 << 30;
/// Releases kept, as `scripts/release.sh` keeps them.
const KEEP: usize = 3;
/// macOS always has these; on Linux they're wherever PATH finds them.
#[cfg(not(windows))]
const CURL: &str = if cfg!(target_os = "macos") { "/usr/bin/curl" } else { "curl" };
#[cfg(not(windows))]
const TAR: &str = if cfg!(target_os = "macos") { "/usr/bin/tar" } else { "tar" };

/// `curl` or `tar` (bsdtar), as this system has it: on Windows only
/// Windows' own, in its system folder as the system names it, never one
/// PATH finds (an agent can write to folders on this user's PATH). Err
/// says why there's none.
fn system_program(name: &str) -> Result<std::ffi::OsString, String> {
    #[cfg(not(windows))]
    {
        Ok(if name == "curl" { CURL } else { TAR }.into())
    }
    #[cfg(windows)]
    {
        let folder = factory_agent::windows::system::system_folder().ok_or("Windows' system folder isn't known")?;
        let program = folder.join(format!("{name}.exe"));
        if program.is_file() { Ok(program.into_os_string()) } else { Err(format!("{} isn't on this computer", program.display())) }
    }
}
/// In a release with no app (a Linux one), where its Mac app is: the Mac
/// tarball of the same release, as the manifest named it, for apps paired
/// from a Mac to update themselves (`crate::app_archive`).
pub(crate) const APP_SOURCE: &str = "app-source.json";
/// In a release this channel downloaded: the manifest's address it came
/// from. Such a release goes again when the channel stops offering it
/// (`Channel::withdraw`); one built here never does.
const DOWNLOADED_FROM: &str = "downloaded-from";
/// In a release `scripts/release.sh` built here: the commit it was built
/// from. A tarball never has it, so a release without one was downloaded,
/// by a backend from before `DOWNLOADED_FROM` or by the online installer.
const BUILT_FROM: &str = "source";
/// In the home: the newest stamped manifest accepted, by channel address
/// (`Seen`).
const ISSUED: &str = "update-channel.json";

/// What `ISSUED` holds.
#[derive(Debug, Default, Deserialize, Serialize)]
struct Seen {
    #[serde(default)]
    channels: BTreeMap<String, Stamp>,
}

/// A stamped manifest accepted: its stamp, and its bytes' SHA-256, so the
/// same manifest read again is told from another stamped the same.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct Stamp {
    issued_ms: u64,
    sha256: String,
}

/// The stamps accepted so far; none when the file is missing or unreadable
/// (unstamped manifests are taken again then, as before stamps).
fn read_seen(home: &Home) -> Seen {
    let path = home.root().join(ISSUED);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(%error, path = %path.display(), "the update channel's stamps couldn't be read; starting again");
            Seen::default()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Seen::default(),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "the update channel's stamps couldn't be read; starting again");
            Seen::default()
        }
    }
}

/// The newest stamp a manifest from `url` must not go back past: this
/// address's own, or, for an https address that has none yet, the newest of
/// any https address's. Every https channel is signed by the same pinned
/// keys, so a channel that moves (the GitHub organization renamed, say)
/// carries its floor along instead of starting afresh and taking an older
/// signed manifest once. A folder's channel (`file://`, the dev Claktory's)
/// keeps only its own.
fn floor<'a>(seen: &'a Seen, url: &str) -> Option<&'a Stamp> {
    if let Some(own) = seen.channels.get(url) {
        return Some(own);
    }
    let https = |address: &str| address.starts_with("https://");
    if !https(url) {
        return None;
    }
    seen.channels.iter().filter(|(other, _)| https(other)).map(|(_, stamp)| stamp).max_by_key(|stamp| stamp.issued_ms)
}

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
    /// The Windows releases, by architecture as Rust names it ("x86_64",
    /// "aarch64"): `factoryd.exe`, its helpers, `tools/`, the app in `app/`
    /// and `backend-version` in a tarball. Empty when the release has none;
    /// a manifest from before them has none, and older backends don't read
    /// it.
    #[serde(default)]
    pub windows: BTreeMap<String, Tarball>,
    /// When it was published, in ms since 1970, as `scripts/publish.sh`
    /// stamps it (strictly increasing on a channel); None in manifests from
    /// before stamps. Compared only with other manifests' stamps, never
    /// with this machine's clock.
    #[serde(default)]
    pub issued_ms: Option<u64>,
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
    /// The manifest isn't signed by a release key: nothing in it is used.
    /// Tried again at the next check (it may be mid-publish).
    #[error("the manifest isn't signed by Claktory's release key: {0}")]
    Unsigned(String),
    /// The manifest is older than one already accepted from the channel (a
    /// replay, or a stale cache): nothing in it is used.
    #[error("the manifest is older than one already read: {0}")]
    Stale(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// What reading the manifest found.
#[derive(Debug, PartialEq)]
pub enum Found {
    UpToDate,
    /// A newer release this machine can't run, or one already rejected.
    Skipped { version: String, why: String },
    /// A newer release to download, and its tarball for this machine.
    Newer(Box<Manifest>, Tarball),
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
        for (arch, tarball) in &mut manifest.windows {
            if let Err(reason) = tarball.check() {
                return bad(&format!("Windows {arch}: {reason}"));
            }
        }
        Ok(manifest)
    }

    /// The tarball for this machine; None when the release has none.
    pub fn here(&self) -> Option<Tarball> {
        if cfg!(target_os = "linux") {
            return self.linux.get(std::env::consts::ARCH).cloned();
        }
        // Its architecture's Windows build; the Mac's is never this machine's.
        if cfg!(windows) {
            return self.windows.get(std::env::consts::ARCH).cloned();
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

/// The first link (a symbolic link, a junction, a file with more than one
/// name) in what a Windows release unpacked to, if any.
#[cfg(windows)]
fn linked_entry(dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { return Some(at) };
        for entry in entries {
            let Ok(entry) = entry else { return Some(at) };
            let path = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else { return Some(path) };
            let identity = crate::windows::fs::identity(&path);
            if meta.file_type().is_symlink() || identity.as_ref().map_or(true, |identity| identity.links > 1 && meta.is_file()) {
                return Some(path);
            }
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    None
}

/// Why this machine can't run the release; None when it can.
async fn unsupported(manifest: &Manifest) -> Option<String> {
    // Windows' own curl and tar fetch and unpack it, or nothing does.
    if cfg!(windows) {
        return ["curl", "tar"].into_iter().find_map(|name| system_program(name).err());
    }
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
    let curl = system_program("curl").map_err(ChannelError::Fetch)?;
    let mut command = tokio::process::Command::new(curl);
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
    tokio::task::spawn_blocking(move || crate::encoding::sha256_file(&path, 1 << 16)).await.map_err(std::io::Error::other)?
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
    /// A release found unusable, by version, and why: it isn't downloaded
    /// again.
    rejected: Option<(String, String)>,
    /// The keys its manifest must be signed by; None for a channel in a
    /// folder, whose manifest needn't be (`requires_signature`).
    keys: Option<Vec<(String, VerifyingKey)>>,
}

impl Channel {
    pub fn new(home: Home, url: impl Into<String>) -> Channel {
        let url = url.into();
        let keys = requires_signature(&url).then(release_keys);
        Channel { home, url, rejected: None, keys }
    }

    /// Requires its manifest to be signed by one of `keys`, wherever it is.
    pub fn signed_by(mut self, keys: Vec<(String, VerifyingKey)>) -> Channel {
        self.keys = Some(keys);
        self
    }

    /// Err unless the manifest's signature (`<url>.sig`) is one of the
    /// keys', when it needs one.
    async fn check_signature(&self, manifest: &[u8]) -> Result<(), ChannelError> {
        let Some(keys) = &self.keys else { return Ok(()) };
        let sidecar = curl(&format!("{}.sig", self.url), MANIFEST_TIMEOUT_SECONDS, SIGNATURE_MAX_BYTES, None)
            .await
            .map_err(|error| ChannelError::Unsigned(format!("its signature couldn't be fetched ({error})")))?;
        verify_manifest(manifest, &sidecar, keys).map_err(ChannelError::Unsigned)
    }

    /// Checks now, and downloads a newer release if there is one.
    pub async fn check(&mut self) -> Result<Checked, ChannelError> {
        match self.find().await? {
            Found::UpToDate => Ok(Checked::UpToDate),
            Found::Skipped { version, .. } => Ok(Checked::Skipped(version)),
            Found::Newer(manifest, tarball) => self.fetch(&manifest, &tarball).await,
        }
    }

    /// Reads the manifest: whether it offers a newer release this machine
    /// can run, without downloading it (`fetch` does).
    pub async fn find(&mut self) -> Result<Found, ChannelError> {
        let bytes = curl(&self.url, MANIFEST_TIMEOUT_SECONDS, MANIFEST_MAX_BYTES, None).await?;
        self.check_signature(&bytes).await?;
        let manifest = Manifest::parse(&bytes)?;
        self.admit(&manifest, &bytes)?;
        self.withdraw(&manifest.version, manifest.issued_ms, VERSION).await;
        let here = latest(&self.home).await.version;
        // With no release here yet, or only older ones (a newer one was
        // withdrawn), the one running is what's here.
        let here = if VERSION != "dev" && (here.is_empty() || newer(VERSION, &here)) { VERSION.to_owned() } else { here };
        if !here.is_empty() && !newer(&manifest.version, &here) {
            return Ok(Found::UpToDate);
        }
        if let Some((version, why)) = &self.rejected
            && *version == manifest.version
        {
            return Ok(Found::Skipped { version: version.clone(), why: why.clone() });
        }
        match (unsupported(&manifest).await, manifest.here()) {
            (None, Some(tarball)) => Ok(Found::Newer(Box::new(manifest), tarball)),
            (reason, _) => {
                let system = if cfg!(windows) { "Windows" } else { "Linux" };
                let why = reason.unwrap_or_else(|| format!("it has no build for {system} {}", std::env::consts::ARCH));
                tracing::info!(version = %manifest.version, %why, "a newer release can't run here");
                self.rejected = Some((manifest.version.clone(), why.clone()));
                Ok(Found::Skipped { version: manifest.version, why })
            }
        }
    }

    /// Err when the manifest (signed, or from a folder) is older than one
    /// already accepted from this channel; otherwise keeps its stamp as the
    /// newest, if it is.
    fn admit(&self, manifest: &Manifest, bytes: &[u8]) -> Result<(), ChannelError> {
        use sha2::Digest;
        let sha256 = crate::encoding::hex(&sha2::Sha256::digest(bytes));
        let mut seen = read_seen(&self.home);
        let newest = floor(&seen, &self.url);
        let stale = match (manifest.issued_ms, newest) {
            (_, None) => None,
            (None, Some(_)) => Some("it isn't stamped, and this channel's are"),
            (Some(issued), Some(newest)) if issued < newest.issued_ms => Some("it was issued before one already read"),
            (Some(issued), Some(newest)) if issued == newest.issued_ms && sha256 != newest.sha256 => {
                Some("another manifest was issued at the same time")
            }
            _ => None,
        };
        if let Some(why) = stale {
            tracing::warn!(version = %manifest.version, issued_ms = ?manifest.issued_ms, why, "refused a stale update manifest");
            return Err(ChannelError::Stale(why.to_owned()));
        }
        if let Some(issued) = manifest.issued_ms
            && newest.is_none_or(|newest| issued > newest.issued_ms)
        {
            seen.channels.insert(self.url.clone(), Stamp { issued_ms: issued, sha256 });
            let written = serde_json::to_vec_pretty(&seen)
                .map_err(std::io::Error::other)
                .and_then(|json| {
                    std::fs::create_dir_all(self.home.root())?;
                    crate::remote::write_private(&self.home.root().join(ISSUED), &json)
                });
            if let Err(error) = written {
                tracing::warn!(%error, "couldn't keep the update channel's newest stamp");
            }
        }
        Ok(())
    }

    /// Downloads the release `find` found.
    pub async fn fetch(&mut self, manifest: &Manifest, tarball: &Tarball) -> Result<Checked, ChannelError> {
        match self.download(manifest, tarball).await {
            Err(ChannelError::Rejected { version, reason }) => {
                self.rejected = Some((version.clone(), reason.clone()));
                Err(ChannelError::Rejected { version, reason })
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
        self.prune(VERSION).await;
        Ok(Checked::Downloaded(version.clone()))
    }

    async fn unpack(&self, manifest: &Manifest, expected: &Tarball, tarball: &Path, partial: &Path) -> Result<(), ChannelError> {
        let rejected = |reason: String| ChannelError::Rejected { version: manifest.version.clone(), reason };
        if let Err(reason) = download_checked(expected, tarball).await? {
            return Err(rejected(reason));
        }
        tokio::fs::create_dir_all(partial).await?;
        // tar keeps the bundle's symlinks and modes, and refuses paths outside the folder.
        let tar = system_program("tar").map_err(|why| rejected(format!("it can't be unpacked here: {why}")))?;
        let untar = tokio::process::Command::new(tar).arg("-xzf").arg(tarball).arg("-C").arg(partial).output().await?;
        if !untar.status.success() {
            return Err(rejected(format!("it doesn't unpack: {}", String::from_utf8_lossy(&untar.stderr).trim())));
        }
        // Windows releases carry no links: any link, or a file with a
        // second name, means it isn't one of ours.
        #[cfg(windows)]
        if let Some(link) = tokio::task::spawn_blocking({
            let partial = partial.to_path_buf();
            move || linked_entry(&partial)
        })
        .await
        .map_err(std::io::Error::other)?
        {
            return Err(rejected(format!("it holds a link ({})", link.display())));
        }
        let backend = crate::home::program("factoryd");
        if !tokio::fs::metadata(partial.join(&backend)).await.is_ok_and(|metadata| metadata.is_file()) {
            return Err(rejected(format!("it has no {backend}")));
        }
        // Every program of Claktory's in it (not tools\) signed under this
        // backend's own certificate profile, when this backend is signed.
        #[cfg(windows)]
        if let Err(why) = tokio::task::spawn_blocking({
            let partial = partial.to_path_buf();
            move || crate::windows::authenticode::check_release(&partial)
        })
        .await
        .map_err(std::io::Error::other)?
        {
            return Err(rejected(format!("it isn't signed as Claktory's own: {why}")));
        }
        // A Linux release has no app: the app runs on another machine.
        if cfg!(target_os = "macos") {
            // From a signed channel, by a backend a Developer ID signed: only
            // code its own team signed.
            let team = if self.keys.is_some() { own_team().await } else { None };
            let codesign = |deep: bool| {
                let mut command = tokio::process::Command::new("/usr/bin/codesign");
                command.args(["--verify", "--strict"]);
                if deep {
                    command.arg("--deep");
                }
                if let Some(team) = &team {
                    command.arg(team_requirement(team));
                }
                command
            };
            let Some(app) = find_app(partial).await? else { return Err(rejected("it has no app".into())) };
            let verify = codesign(true).arg(&app).output().await?;
            if !verify.status.success() {
                return Err(rejected(format!("its app's signature doesn't verify: {}", String::from_utf8_lossy(&verify.stderr).trim())));
            }
            if team.is_some() {
                let verify = codesign(false).arg(partial.join("factoryd")).output().await?;
                if !verify.status.success() {
                    let why = String::from_utf8_lossy(&verify.stderr);
                    return Err(rejected(format!("its backend's signature doesn't verify: {}", why.trim())));
                }
            }
            // The helpers beside factoryd (the apps host, the speech host,
            // the video preview maker; the WSL helper only in Linux ones),
            // which releases from before them don't have.
            for name in crate::update::HELPERS {
                let label = match name {
                    crate::update::APPS_HOST => "apps host",
                    crate::update::SPEECH_HOST => "speech host",
                    crate::update::WSL_HELPER => "WSL helper",
                    _ => "video preview maker",
                };
                let helper = partial.join(name);
                if tokio::fs::metadata(&helper).await.is_ok_and(|metadata| metadata.is_file()) {
                    let verify = codesign(false).arg(&helper).output().await?;
                    if !verify.status.success() {
                        let why = String::from_utf8_lossy(&verify.stderr);
                        return Err(rejected(format!("its {label}'s signature doesn't verify: {}", why.trim())));
                    }
                }
            }
        } else {
            // Its app is the Mac release's, fetched when a paired app asks.
            let mac = Tarball { url: manifest.url.clone(), sha256: manifest.sha256.clone(), size: manifest.size };
            tokio::fs::write(partial.join(APP_SOURCE), serde_json::to_vec(&mac).map_err(std::io::Error::other)?).await?;
        }
        tokio::fs::write(partial.join("backend-version"), format!("{}\n", manifest.backend)).await?;
        // The channel, then the stamp of the manifest it came from (none
        // before stamps).
        let stamp = manifest.issued_ms.map(|issued| format!("{issued}\n")).unwrap_or_default();
        tokio::fs::write(partial.join(DOWNLOADED_FROM), format!("{}\n{stamp}", self.url)).await?;
        Ok(())
    }

    /// Forgets the releases this channel gave that it no longer offers: a
    /// release pulled after it was downloaded (the manifest, stamped
    /// `issued` after the one it came from, moved back to `listed`, older
    /// than it) is deleted, so it's no longer offered, and `releases/latest`
    /// names the newest release left. Only releases newer than the backend
    /// `running` go: never the one running, nor the one an update would go
    /// back to, nor a release built here. An unstamped manifest withdraws
    /// nothing.
    async fn withdraw(&self, listed: &str, issued: Option<u64>, running: &str) {
        let Some(issued) = issued else { return };
        let releases = self.home.releases();
        let Ok(mut entries) = tokio::fs::read_dir(&releases).await else { return };
        let mut left = Vec::new();
        let mut pulled = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !plain(&name) || name.starts_with('.') || name.starts_with("latest") || !entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let dir = entry.path();
            // The stamp of the manifest it came from: none when it came
            // from an unstamped one, or by an older backend (no mark).
            let marker = tokio::fs::read_to_string(dir.join(DOWNLOADED_FROM)).await.ok();
            let came_from = marker.as_deref().and_then(|text| text.lines().nth(1)).and_then(|line| line.trim().parse::<u64>().ok());
            let downloaded = marker.is_some() || !tokio::fs::try_exists(dir.join(BUILT_FROM)).await.unwrap_or(true);
            // An unstamped backend ("dev") is older than any release, as `find` takes it.
            let after_running = name != running && (running == "dev" || newer(&name, running));
            if downloaded && after_running && newer(&name, listed) && came_from.is_none_or(|stamp| stamp < issued) {
                pulled.push(name);
            } else if dir.join(crate::home::program("factoryd")).is_file() {
                left.push(name);
            }
        }
        if pulled.is_empty() {
            return;
        }
        // `latest` first, so it never names a release that's gone.
        let named = tokio::fs::read_to_string(releases.join("latest")).await.unwrap_or_default();
        if pulled.iter().any(|version| version == named.trim()) {
            left.sort_unstable_by(|a, b| crate::update::compare_versions(b, a));
            let renamed = match left.first() {
                Some(newest) => {
                    let next = releases.join("latest.new");
                    match tokio::fs::write(&next, format!("{newest}\n")).await {
                        Ok(()) => tokio::fs::rename(&next, releases.join("latest")).await,
                        Err(error) => Err(error),
                    }
                }
                None => tokio::fs::remove_file(releases.join("latest")).await,
            };
            if let Err(error) = renamed {
                tracing::warn!(%error, "could not name the newest release left");
                return;
            }
        }
        for version in &pulled {
            tracing::info!(%version, channel = %listed, "the channel no longer offers a release downloaded from it; deleting it");
            remove_all(&releases.join(version)).await;
        }
    }

    /// Keeps the newest few releases, and always the one `latest` names,
    /// the one `running` (whose helpers a start heals from) and the one a
    /// rollback would go back to.
    async fn prune(&self, running: &str) {
        let releases = self.home.releases();
        let named = latest(&self.home).await.version;
        let rollback = crate::update::rollback_version(&self.home);
        let Ok(mut entries) = tokio::fs::read_dir(&releases).await else { return };
        let mut versions = Vec::new();
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name().to_string_lossy().into_owned();
            if plain(&name) && !name.starts_with("latest") && entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
                versions.push(name);
            }
        }
        versions.sort_unstable_by(|a, b| crate::update::compare_versions(b, a));
        let kept = |version: &String| *version == named || version == running || rollback.as_ref() == Some(version);
        for old in versions.into_iter().skip(KEEP).filter(|version| !kept(version)) {
            remove_all(&releases.join(old)).await;
        }
    }
}

/// Checks the channel shortly after start and every half hour after, while
/// the backend runs. A failed check is tried again at the next one; the
/// person sees nothing until a release is complete, unless they asked
/// (`Factory::check_for_updates`), whose answer it keeps current.
pub async fn poll(factory: std::sync::Arc<crate::factory::Factory>) {
    tokio::time::sleep(FIRST_CHECK).await;
    loop {
        factory.check_on_schedule().await;
        tokio::time::sleep(EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_moved_https_channel_keeps_the_newest_stamp_and_a_folders_keeps_its_own() {
        use super::{Seen, Stamp, floor};
        let stamp = |issued_ms: u64| Stamp { issued_ms, sha256: format!("{issued_ms}") };
        let mut seen = Seen::default();
        let old = "https://github.com/claktor/claktory/releases/latest/download/latest.json";
        let new = "https://github.com/claktory/claktory/releases/latest/download/latest.json";
        seen.channels.insert(old.to_owned(), stamp(3000));
        seen.channels.insert("https://example.com/other/latest.json".to_owned(), stamp(2000));
        seen.channels.insert("file:///tmp/channel/latest.json".to_owned(), stamp(9000));
        // A new https address starts at the newest https stamp, never a folder's.
        assert_eq!(floor(&seen, new), Some(&stamp(3000)));
        // Its own stamp, once it has one, is what counts.
        seen.channels.insert(new.to_owned(), stamp(4000));
        assert_eq!(floor(&seen, new), Some(&stamp(4000)));
        // A folder's channel keeps only its own, and starts afresh.
        assert_eq!(floor(&seen, "file:///tmp/channel/latest.json"), Some(&stamp(9000)));
        assert_eq!(floor(&seen, "file:///tmp/another/latest.json"), None);
        assert_eq!(floor(&Seen::default(), new), None);
    }

    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_bytes(&[seed; 32])
    }

    fn sidecar(id: &str, key: &SigningKey, bytes: &[u8]) -> Vec<u8> {
        format!("{id}:{}\n", STANDARD.encode(key.sign(bytes).to_bytes())).into_bytes()
    }

    fn pinned() -> Vec<(String, VerifyingKey)> {
        vec![("claktory-release-1".into(), key(1).verifying_key()), ("claktory-recovery-1".into(), key(2).verifying_key())]
    }

    #[test]
    fn a_manifest_signed_by_a_pinned_key_verifies_and_nothing_else_does() {
        let bytes = br#"{"version":"v0.1.9"}"#;
        assert_eq!(verify_manifest(bytes, &sidecar("claktory-release-1", &key(1), bytes), &pinned()), Ok(()));
        // The recovery key works too, by its own id.
        assert_eq!(verify_manifest(bytes, &sidecar("claktory-recovery-1", &key(2), bytes), &pinned()), Ok(()));
        // Other bytes than were signed: a changed manifest.
        let changed = br#"{"version":"v0.1.99"}"#;
        assert!(verify_manifest(changed, &sidecar("claktory-release-1", &key(1), bytes), &pinned()).is_err());
        // A key nobody pinned, under a pinned id and under its own.
        assert!(verify_manifest(bytes, &sidecar("claktory-release-1", &key(3), bytes), &pinned()).is_err());
        assert!(verify_manifest(bytes, &sidecar("someone-else", &key(3), bytes), &pinned()).is_err());
        // One pinned key's signature under the other's id.
        assert!(verify_manifest(bytes, &sidecar("claktory-recovery-1", &key(1), bytes), &pinned()).is_err());
        // Malformed sidecars.
        for bad in [&b""[..], b"claktory-release-1", b"claktory-release-1:not base64!", b"claktory-release-1:AAAA", b"\xff\xfe"] {
            assert!(verify_manifest(bytes, bad, &pinned()).is_err(), "{bad:?}");
        }
        // No keys at all: nothing verifies.
        assert!(verify_manifest(bytes, &sidecar("claktory-release-1", &key(1), bytes), &[]).is_err());
    }

    /// What `scripts/publish.sh` writes (OpenSSL 3's `pkeyutl -sign -rawin`,
    /// pure Ed25519) verifies here: a throwaway key, made once for this test.
    #[test]
    fn a_signature_made_with_openssl_verifies() {
        let manifest = br#"{"version":"v0.1.9","note":"interop fixture"}"#;
        let key = public_key("8crb3Obm8e2Xkauy1SvgpKQxzHdTzLGwHJopYdDnX3w=").expect("key");
        let sidecar = b"claktory-release-1:cnAZ3VpFcWU5mRJzz8WMr/HlnwZbUVDV6lYqFfhTixLrBrINVZG8f3i2RM1HZ0/mLjfD3dbWT+CZh6O1pgABBg==\n";
        assert_eq!(verify_manifest(manifest, sidecar, &[("claktory-release-1".into(), key)]), Ok(()));
        assert!(verify_manifest(b"{}", sidecar, &[("claktory-release-1".into(), key)]).is_err());
    }

    #[test]
    fn the_pinned_release_keys_all_read() {
        // Never none: with no keys every online update is refused, and "none"
        // must never come to mean "don't check".
        assert!(!RELEASE_KEYS.is_empty(), "release keys are pinned");
        assert_eq!(release_keys().len(), RELEASE_KEYS.len(), "every pinned key is a valid ed25519 public key");
        let mut ids: Vec<&str> = RELEASE_KEYS.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), RELEASE_KEYS.len(), "key ids are unique");
        assert_eq!(public_key(&STANDARD.encode(key(1).verifying_key().to_bytes())), Some(key(1).verifying_key()));
        assert_eq!(public_key("AAAA"), None);
    }

    #[test]
    fn only_a_channel_in_a_folder_goes_unsigned() {
        assert!(!requires_signature("file:///tmp/channel/latest.json"));
        assert!(requires_signature("https://github.com/claktor/claktory/releases/latest/download/latest.json"));
        assert!(requires_signature("http://example.com/latest.json"));
        assert!(requires_signature("FILE:///tmp/latest.json"), "anything that isn't exactly a folder's is signed");
    }

    #[test]
    fn the_team_comes_from_a_developer_id_signature_only() {
        assert_eq!(team_in("Identifier=dev.factory.factoryd\nTeamIdentifier=AB12CD34EF\n"), Some("AB12CD34EF".into()));
        assert_eq!(team_in("Signature=adhoc\nTeamIdentifier=not set\n"), None);
        assert_eq!(team_in("code object is not signed at all"), None);
        assert_eq!(team_in("TeamIdentifier=AB\" or 1=1"), None);
    }

    /// codesign takes the requirement as it's written, and an app signed ad
    /// hoc (which `--verify` alone accepts) doesn't meet it.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_team_requirement_turns_away_code_signed_ad_hoc() {
        let dir = std::env::temp_dir().join(format!("factoryd-team-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let binary = dir.join("adhoc");
        std::fs::copy("/usr/bin/true", &binary).expect("copy");
        let signed = std::process::Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&binary).output().expect("sign");
        assert!(signed.status.success());
        let plain = std::process::Command::new("/usr/bin/codesign").args(["--verify", "--strict"]).arg(&binary).status().expect("verify");
        assert!(plain.success(), "ad hoc passes a plain verify");
        let pinned = std::process::Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict"])
            .arg(team_requirement("AB12CD34EF"))
            .arg(&binary)
            .output()
            .expect("verify");
        assert!(!pinned.status.success(), "the team requirement refuses it");
        assert!(String::from_utf8_lossy(&pinned.stderr).contains("requirement"), "refused for the requirement, not its syntax");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A release the channel pulled goes only when it's newer than the
    /// backend running: the running one, and older ones an update would go
    /// back to, stay even when they came from the channel.
    #[tokio::test]
    async fn a_pulled_release_goes_only_when_newer_than_the_one_running() {
        let dir = std::env::temp_dir().join(format!("factoryd-withdraw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = Home::new(dir.clone());
        let releases = home.releases();
        for version in ["v0.1.20", "v0.1.21", "v0.1.22"] {
            std::fs::create_dir_all(releases.join(version)).expect("release");
            std::fs::write(releases.join(version).join(crate::home::program("factoryd")), "backend").expect("factoryd");
            std::fs::write(releases.join(version).join(DOWNLOADED_FROM), "file:///channel/latest.json\n").expect("marker");
        }
        // One from a manifest stamped after the one that moves back.
        std::fs::create_dir_all(releases.join("v0.1.23")).expect("release");
        std::fs::write(releases.join("v0.1.23").join(crate::home::program("factoryd")), "backend").expect("factoryd");
        std::fs::write(releases.join("v0.1.23").join(DOWNLOADED_FROM), "file:///channel/latest.json\n5\n").expect("marker");
        std::fs::write(releases.join("latest"), "v0.1.22\n").expect("latest");
        let channel = Channel::new(home.clone(), "file:///channel/latest.json");
        let left = || {
            let mut left: Vec<String> =
                std::fs::read_dir(&releases).expect("releases").map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned()).collect();
            left.sort();
            left
        };

        // Unstamped: nothing goes.
        channel.withdraw("v0.1.19", None, "v0.1.21").await;
        assert_eq!(left(), ["latest", "v0.1.20", "v0.1.21", "v0.1.22", "v0.1.23"]);

        // The channel went back past the one running (v0.1.21): it stays,
        // and so does the older one, and the one stamped later.
        channel.withdraw("v0.1.19", Some(2), "v0.1.21").await;
        assert_eq!(left(), ["latest", "v0.1.20", "v0.1.21", "v0.1.23"]);
        assert_eq!(latest(&home).await.version, "v0.1.23");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Pruning keeps the newest few, and never the running release's folder
    /// (a start heals its helpers from it) or the one a rollback goes to.
    #[tokio::test]
    async fn pruning_keeps_the_running_release_and_the_rollbacks() {
        let dir = std::env::temp_dir().join(format!("factoryd-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = Home::new(dir.clone());
        let releases = home.releases();
        for version in ["v0.1.27", "v0.1.28", "v0.1.30", "v0.1.32", "v0.1.33", "v0.1.34"] {
            std::fs::create_dir_all(releases.join(version)).expect("release");
            std::fs::write(releases.join(version).join(crate::home::program("factoryd")), "backend").expect("factoryd");
        }
        std::fs::write(releases.join("latest"), "v0.1.34\n").expect("latest");
        // On trial: v0.1.30 runs, and a rollback would go back to v0.1.28.
        std::fs::create_dir_all(dir.join("bin")).expect("bin");
        std::fs::write(home.trial_path(), r#"{"from": "v0.1.28", "to": "v0.1.30", "started": true}"#).expect("trial");
        let channel = Channel::new(home.clone(), "file:///channel/latest.json");
        channel.prune("v0.1.30").await;
        let mut left: Vec<String> =
            std::fs::read_dir(&releases).expect("releases").map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["latest", "v0.1.28", "v0.1.30", "v0.1.32", "v0.1.33", "v0.1.34"]);

        // Off trial, only the running one is kept beyond the newest three.
        std::fs::remove_file(home.trial_path()).expect("trial");
        channel.prune("v0.1.30").await;
        let mut left: Vec<String> =
            std::fs::read_dir(&releases).expect("releases").map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["latest", "v0.1.30", "v0.1.32", "v0.1.33", "v0.1.34"]);
        let _ = std::fs::remove_dir_all(dir);
    }

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

    /// A Windows backend takes its architecture's `windows` entry, never the
    /// Mac tarball; a manifest without the key (all before it) still reads,
    /// and offers a Windows backend nothing.
    #[test]
    fn windows_releases_are_checked_and_taken_only_on_windows() {
        let before = manifest(good()).expect("a manifest without windows");
        assert!(before.windows.is_empty());
        if cfg!(windows) {
            assert_eq!(before.here(), None, "the Mac's isn't Windows's");
        }
        let mut with = good();
        with["windows"] = serde_json::json!({std::env::consts::ARCH: {"url": "https://example.com/windows.tar.gz", "sha256": "EF".repeat(32), "size": 10}});
        let parsed = manifest(with.clone()).expect("manifest");
        assert_eq!(parsed.windows[std::env::consts::ARCH].sha256, "ef".repeat(32));
        let here = parsed.here().map(|tarball| tarball.url);
        if cfg!(windows) {
            assert_eq!(here.as_deref(), Some("https://example.com/windows.tar.gz"));
        } else if cfg!(target_os = "macos") {
            assert_eq!(here, Some(parsed.url.clone()), "a Mac never takes Windows's");
        }
        with["windows"][std::env::consts::ARCH]["url"] = serde_json::json!("http://example.com/windows.tar.gz");
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

    #[test]
    fn macos_versions_compare_by_major_then_minor() {
        assert_eq!(macos_version("15.7.3"), Some((15, 7)));
        assert_eq!(macos_version("12"), Some((12, 0)));
        assert_eq!(macos_version(""), None);
        assert!(macos_version("11.6").expect("version") < macos_version("12.0").expect("version"));
    }
}
