//! Updates, and getting back from a bad one.
//!
//! `scripts/release.sh` publishes a build into `<home>/releases/<version>/`: the
//! app bundle, the backend, and `backend-version`, which names an older
//! backend when the release only changed the app. Then it names the release in
//! `<home>/releases/latest`. Factory notices, the app offers the update, and
//! installing it replaces the app, and the backend when it's newer.
//!
//! The backend runs under a supervisor (`factoryd supervise`, installed once
//! and never updated). To update, the backend keeps itself as
//! `bin/factoryd.previous`, puts the new one in `bin/factoryd`, writes
//! `bin/trial.json` and exits; the supervisor starts the new one. The new
//! backend passes its trial by serving the app (or, with no app around, by
//! staying up). If it dies first, however it dies, the supervisor puts the
//! previous backend back, and Factory says the update failed.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::factory::{Factory, RequestError};
use crate::home::Home;

/// This build's version, stamped by the build scripts; "dev" otherwise.
pub const VERSION: &str = match option_env!("FACTORY_VERSION") {
    Some(version) => version,
    None => "dev",
};

/// Set by the supervisor for the backend it runs.
pub const SUPERVISED: &str = "FACTORY_SUPERVISED";

/// What the supervisor (`factoryd supervise`) does, as a number: bump it
/// whenever that changes (`supervise_backend` in main.rs, and what it calls
/// here: `start_trial`, `on_trial`, `update_failed`, `roll_back`,
/// `fall_back`). An update moves the LaunchAgent to its release's
/// supervisor, which restarts the backend a second time, only when the
/// installed supervisor's number is lower (`factoryd supervisor-version`);
/// supervisors from before it don't answer, and count as 0.
pub const SUPERVISOR_VERSION: u32 = 1;

/// Time for the ack to reach the app before the backend hands over.
const RESTART_DELAY: Duration = Duration::from_millis(300);

/// How long a new backend has to serve an app that's trying to connect.
const TRIAL: Duration = Duration::from_secs(30);

/// Waits for the app to quit (forcing it after 10s), swaps the bundle (the
/// old one moved aside until the new one is in place), and reopens it;
/// Factory Dev in the background, as always.
const SWAP_APP: &str = r#"app="$1"; new="$2"
(
  for _ in $(seq 100); do pgrep -f "$app/Contents/MacOS/" >/dev/null || break; sleep 0.1; done
  pkill -f "$app/Contents/MacOS/"
  rm -rf "$app.updating" "$app.previous" && ditto "$new" "$app.updating" || exit 1
  # Moved aside, not deleted first: there's always an app, but for the
  # instant between two renames.
  mv "$app" "$app.previous" && mv "$app.updating" "$app" || { [ -d "$app" ] || mv "$app.previous" "$app"; exit 1; }
  rm -rf "$app.previous"
  case "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist")" in
    *.dev) open -g "$app" ;;
    *) open "$app" ;;
  esac
) </dev/null >/dev/null 2>&1 &"#;

/// The newest complete release.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Release {
    /// "" when none is published.
    pub version: String,
    /// The backend in it; older than `version` when only the app changed.
    pub backend: String,
    /// Its changelog (`CHANGELOG.md`), newest first: every version up to
    /// this one. Empty for releases made before releases carried one.
    pub changelog: Vec<ChangelogVersion>,
}

/// What one version added and fixed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangelogVersion {
    /// "" for the entries not yet released (only a dev Factory's releases have them).
    pub version: String,
    /// When it was released, in ms since 1970: from its "Released" line, or
    /// its date-stamped version; 0 if unknown.
    pub released: i64,
    pub items: Vec<ChangelogItem>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ItemKind {
    #[default]
    Other,
    Added,
    Changed,
    Fixed,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangelogItem {
    pub kind: ItemKind,
    /// Without its "Added"/"Changed:"/"Fixed:" in front, capitalized.
    pub text: String,
    /// As written.
    pub raw: String,
}

impl ChangelogVersion {
    pub(crate) fn to_proto(&self) -> factory_protocol::v1::ChangelogVersion {
        use factory_protocol::v1::{ChangelogItem as Item, ChangelogKind as Kind};
        let items = self
            .items
            .iter()
            .map(|item| {
                let kind = match item.kind {
                    ItemKind::Other => Kind::Other,
                    ItemKind::Added => Kind::Added,
                    ItemKind::Changed => Kind::Changed,
                    ItemKind::Fixed => Kind::Fixed,
                };
                Item { kind: kind.into(), text: item.text.clone(), raw: item.raw.clone() }
            })
            .collect();
        factory_protocol::v1::ChangelogVersion {
            version: self.version.clone(),
            released: self.released,
            items,
            unreleased: self.version.is_empty(),
        }
    }
}

/// Reads docs/CHANGELOG.md's format: `## <version>` headings (`## Unreleased`
/// for what isn't released yet), each followed by `- ` items, newest first.
/// An item's indented lines continue it. Anything else is left out.
pub fn parse_changelog(text: &str) -> Vec<ChangelogVersion> {
    let mut versions: Vec<ChangelogVersion> = Vec::new();
    let mut in_version = false;
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            let heading = heading.trim();
            in_version = heading == "Unreleased" || plain(heading);
            if in_version {
                let version = if heading == "Unreleased" { String::new() } else { heading.to_owned() };
                versions.push(ChangelogVersion { released: version_time(&version), version, items: Vec::new() });
            }
            continue;
        }
        let Some(version) = versions.last_mut().filter(|_| in_version) else { continue };
        if let Some(when) = line.strip_prefix("Released ")
            && version.items.is_empty()
            && let Some(at) = released_at(when)
        {
            version.released = at;
        } else if let Some(item) = line.strip_prefix("- ") {
            version.items.push(ChangelogItem { raw: item.trim().to_owned(), ..ChangelogItem::default() });
        } else if line.starts_with("  ")
            && !line.trim().is_empty()
            && let Some(last) = version.items.last_mut()
        {
            last.raw.push(' ');
            last.raw.push_str(line.trim());
        }
    }
    for item in versions.iter_mut().flat_map(|version| version.items.iter_mut()) {
        let (kind, text) = item_kind(&item.raw);
        item.kind = kind;
        item.text = text;
    }
    // Unreleased without entries says nothing.
    versions.retain(|version| !version.version.is_empty() || !version.items.is_empty());
    versions
}

/// "2026-11-20 10:12 UTC." (the line `scripts/changelog.sh stamp` writes
/// under a numbered version's heading) in ms since 1970.
fn released_at(text: &str) -> Option<i64> {
    let text = text.trim().trim_end_matches('.').strip_suffix(" UTC")?;
    let at: jiff::civil::DateTime = text.parse().ok()?;
    Some(at.to_zoned_datetime(jiff::tz::TimeZone::UTC).ok()?.timestamp().as_millisecond())
}

/// "Added the Tasks panel." -> (Added, "The Tasks panel."); "Fixed: x" -> (Fixed, "X").
fn item_kind(raw: &str) -> (ItemKind, String) {
    for (prefix, kind) in [("Added", ItemKind::Added), ("Changed", ItemKind::Changed), ("Fixed", ItemKind::Fixed)] {
        if let Some(rest) = raw.strip_prefix(prefix)
            && (rest.starts_with(' ') || rest.starts_with(':'))
        {
            let rest = rest.trim_start_matches(':').trim();
            let mut characters = rest.chars();
            if let Some(first) = characters.next() {
                return (kind, first.to_uppercase().chain(characters).collect());
            }
        }
    }
    (ItemKind::Other, raw.to_owned())
}

/// Where a version stands, oldest kinds first: the date stamps Factory used
/// until v0.1.0, then numbered versions, then "dev" (an unstamped build,
/// never offered an update).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Rank<'a> {
    /// "2026.11.19-1958", ordered as text; anything else unknown goes here too.
    Stamp(&'a str),
    /// "v0.1.12": its numbers, and whether it's a release (true) or a build
    /// before it ("v0.1.13-dev.20261120-1012", ordered by what follows "-").
    Numbered([u64; 3], bool, &'a str),
    Dev,
}

/// "v0.1.12" -> 0, 1, 12; "v0.2.0-dev.20261120-1012" -> 0, 2, 0 and its build.
pub fn numbers(version: &str) -> Option<([u64; 3], Option<&str>)> {
    let rest = version.strip_prefix('v')?;
    let (numbers, build) = match rest.split_once('-') {
        Some((numbers, build)) => (numbers, Some(build)),
        None => (rest, None),
    };
    let mut parts =
        numbers.split('.').map(|part| if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) { None } else { part.parse().ok() });
    let found = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some((found, build))
}

fn rank(version: &str) -> Rank<'_> {
    if version == "dev" {
        return Rank::Dev;
    }
    match numbers(version) {
        Some((numbers, None)) => Rank::Numbered(numbers, true, ""),
        Some((numbers, Some(build))) => Rank::Numbered(numbers, false, build),
        None => Rank::Stamp(version),
    }
}

/// Orders versions by when they were made: a date stamp before every
/// numbered version, numbers part by part (v0.1.100 after v0.1.99, v0.2.0
/// after v0.1.500), a dev build before the release it leads to.
///
/// Numbered versions start with "v" so that a backend or app from before them,
/// which compares versions as text, sees v0.1.0 as newer than any date stamp
/// ("v" sorts after digits) and offers the update to it.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    rank(a).cmp(&rank(b))
}

/// Whether `candidate` is newer than `than`.
pub fn newer(candidate: &str, than: &str) -> bool {
    compare_versions(candidate, than).is_gt()
}

/// "2026.11.18-2123" (UTC, as `scripts/lib.sh` stamped them, maybe with `.SS`)
/// in ms since 1970; 0 when it isn't such a version.
pub fn version_time(version: &str) -> i64 {
    let parse = || -> Option<i64> {
        let (date, time) = version.split_once('-')?;
        let mut date = date.split('.');
        let (year, month, day) = (date.next()?.parse().ok()?, date.next()?.parse().ok()?, date.next()?.parse().ok()?);
        let (clock, seconds) = time.split_once('.').unwrap_or((time, "0"));
        if clock.len() != 4 || date.next().is_some() {
            return None;
        }
        let (hour, minute) = (clock[..2].parse().ok()?, clock[2..].parse().ok()?);
        let at = jiff::civil::DateTime::new(year, month, day, hour, minute, seconds.parse().ok()?, 0).ok()?;
        Some(at.to_zoned_datetime(jiff::tz::TimeZone::UTC).ok()?.timestamp().as_millisecond())
    };
    parse().unwrap_or(0)
}

pub async fn latest(home: &Home) -> Release {
    let Ok(text) = tokio::fs::read_to_string(home.releases().join("latest")).await else { return Release::default() };
    let version = text.trim();
    let dir = home.releases().join(version);
    if !plain(version) || !dir.join("factoryd").is_file() {
        return Release::default();
    }
    let backend = match tokio::fs::read_to_string(dir.join("backend-version")).await {
        Ok(backend) if plain(backend.trim()) => backend.trim().to_owned(),
        _ => version.to_owned(),
    };
    let changelog = tokio::fs::read_to_string(dir.join("CHANGELOG.md")).await.map(|text| parse_changelog(&text)).unwrap_or_default();
    Release { version: version.to_owned(), backend, changelog }
}

pub(crate) fn plain(version: &str) -> bool {
    !version.is_empty() && version.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// A backend update that hasn't proven itself yet.
#[derive(Debug, Serialize, Deserialize)]
struct Trial {
    from: String,
    to: String,
    /// The new backend has started at least once.
    #[serde(default)]
    started: bool,
    /// Why it stopped, when it could say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

/// An update that didn't start, which Factory went back from.
#[derive(Debug, Serialize, Deserialize)]
struct Failed {
    version: String,
    back_to: String,
    reason: String,
}

fn read_trial(home: &Home) -> Option<Trial> {
    let bytes = std::fs::read(home.trial_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn remove(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

/// Called by the supervisor before it starts the backend: if an update is on
/// trial, this start is its chance. Marked here rather than by the backend, so
/// even one that can't run at all is caught.
pub fn start_trial(home: &Home) -> std::io::Result<()> {
    let Some(mut trial) = read_trial(home) else { return Ok(()) };
    if trial.started {
        return Ok(());
    }
    trial.started = true;
    write_json(&home.trial_path(), &trial)
}

/// A backend on trial couldn't start; says why, for the supervisor.
pub fn trial_failed(home: &Home, reason: &dyn std::fmt::Display) {
    let Some(mut trial) = read_trial(home) else { return };
    trial.reason = Some(format!("{reason}"));
    if let Err(error) = write_json(&home.trial_path(), &trial) {
        tracing::error!(%error, "could not record why the update failed");
    }
}

/// An update is on trial: the backend started now is new, or the one
/// before it coming back.
pub fn on_trial(home: &Home) -> bool {
    read_trial(home).is_some()
}

/// For the supervisor, after the backend stopped or couldn't be started:
/// whether that was an update failing. A trial that hasn't started yet means
/// the backend stopped to hand over to the update.
pub fn update_failed(home: &Home) -> bool {
    read_trial(home).is_some_and(|trial| trial.started)
}

/// Puts the previous backend back after a failed update, and records why.
pub fn roll_back(home: &Home, how_it_ended: &str) -> std::io::Result<()> {
    let trial = read_trial(home);
    restore_previous(home)?;
    if let Some(trial) = trial {
        let reason = trial.reason.unwrap_or_else(|| how_it_ended.to_owned());
        tracing::warn!(version = %trial.to, back_to = %trial.from, %reason, "update failed; went back");
        write_json(&home.update_failed_path(), &Failed { version: trial.to, back_to: trial.from, reason })?;
    }
    remove(&home.trial_path())
}

/// What the app shows when the last update had to be undone; "" otherwise.
pub fn failure_message(home: &Home) -> String {
    let Ok(bytes) = std::fs::read(home.update_failed_path()) else { return String::new() };
    match serde_json::from_slice::<Failed>(&bytes) {
        Ok(failed) if failed.version.is_empty() => {
            format!("Claktory's backend kept stopping, so it went back to the one before. {}", failed.reason)
        }
        Ok(failed) => {
            format!("Claktory {} didn't start, so you're still on {}. {}", failed.version, failed.back_to, failed.reason)
        }
        Err(_) => String::new(),
    }
}

/// Whether the backend is serving: an app tried to connect, and one got its
/// state. A new backend passes its trial on that.
#[derive(Debug)]
pub struct Health {
    tried: AtomicBool,
    served: watch::Sender<bool>,
}

impl Default for Health {
    fn default() -> Health {
        Health { tried: AtomicBool::new(false), served: watch::Sender::new(false) }
    }
}

impl Health {
    pub(crate) fn connecting(&self) {
        self.tried.store(true, Ordering::Relaxed);
    }

    pub(crate) fn served(&self) {
        self.served.send_replace(true);
    }
}

impl Factory {
    /// Judges a backend on trial: it passes once it serves an app, or when no
    /// app tried to connect within the trial. An app that tried and wasn't
    /// served fails it: the backend exits and the supervisor goes back.
    pub(crate) fn watch_trial(self: &Arc<Self>) {
        if read_trial(&self.home).is_none() {
            return;
        }
        let factory = self.clone();
        tokio::spawn(async move {
            let mut served = factory.health.served.subscribe();
            let passed = tokio::time::timeout(TRIAL, served.wait_for(|served| *served)).await.is_ok();
            if passed || !factory.health.tried.load(Ordering::Relaxed) {
                factory.pass_trial();
            } else {
                trial_failed(&factory.home, &"It couldn't serve the app.");
                crate::account::analytics::backend_stopped(&factory.home);
                std::process::exit(1);
            }
        });
    }

    fn pass_trial(&self) {
        for path in [self.home.trial_path(), self.home.update_failed_path()] {
            if let Err(error) = remove(&path) {
                tracing::error!(%error, path = %path.display(), "could not finish the update");
            }
        }
        tracing::info!(version = VERSION, "update passed its trial");
        // No rollback now: the supervisor forgets the vault's key.
        crate::vault::handoff::release();
        let mut inner = self.lock();
        inner.update_error.clear();
        self.broadcast_workspace(&inner);
    }

    pub(crate) async fn refresh_release(&self) {
        let latest = latest(&self.home).await;
        let mut inner = self.lock();
        if inner.latest_release != latest {
            tracing::info!(version = %latest.version, backend = %latest.backend, "release published");
            // A new release is a new chance: forget the last one that failed.
            if !inner.update_error.is_empty()
                && let Err(error) = remove(&self.home.update_failed_path())
            {
                tracing::error!(%error, "could not clear the failed update");
            }
            inner.update_error.clear();
            inner.latest_release = latest;
            self.broadcast_workspace(&inner);
        }
    }

    pub async fn install_update(self: &Arc<Self>, app_path: &str) -> Result<(), RequestError> {
        let latest = self.lock().latest_release.clone();
        if latest.version.is_empty() {
            return Err(RequestError::NoRelease);
        }
        let release = self.home.releases().join(&latest.version);
        if !app_path.is_empty() {
            let app = Path::new(app_path);
            if !app.extension().is_some_and(|extension| extension == "app") || !app.is_dir() {
                return Err(RequestError::NotAnApp(app_path.to_owned()));
            }
            let Some(new_app) = release_app(&release, app) else { return Err(RequestError::NotAnApp(app_path.to_owned())) };
            // Checked before anything moves, so an app it can't replace stays as it is.
            let folder = app.parent().unwrap_or(Path::new("/"));
            if !writable(folder) {
                return Err(RequestError::AppNotWritable(folder.display().to_string()));
            }
            let status = tokio::process::Command::new("/bin/sh")
                .args(["-c", SWAP_APP, "sh"])
                .arg(app)
                .arg(&new_app)
                // Its own process group, so nothing that stops the backend stops it.
                .process_group(0)
                .status()
                .await?;
            if !status.success() {
                return Err(RequestError::Io(std::io::Error::other("couldn't start replacing the app")));
            }
        }
        // An app-only release leaves the backend, and the agents it's running, alone.
        if latest.backend == VERSION {
            return Ok(());
        }
        let backend = self.home.backend();
        let new_backend = tokio::fs::read(release.join("factoryd")).await?;
        let supervised = std::env::var_os(SUPERVISED).is_some();
        // The helpers first: the backend going in is what makes it count.
        let home = self.home.clone();
        let from = release.clone();
        tokio::task::spawn_blocking(move || stage_helpers(&home, &from)).await.map_err(std::io::Error::other)??;
        let replaced = async {
            if supervised {
                let running = tokio::fs::read(std::env::current_exe()?).await?;
                // The next backend needn't keep a copy of the sign-ins for this one.
                crate::vault::signins::note_backend(&self.home, &running);
                write_durably(self.home.previous_backend(), running).await?;
                let trial = Trial { from: VERSION.to_owned(), to: latest.backend.clone(), started: false, reason: None };
                write_json(&self.home.trial_path(), &trial)?;
            }
            write_durably(backend.clone(), new_backend).await
        };
        if let Err(error) = replaced.await {
            // The backend stays as it was, so its helpers go back too.
            if let Err(error) = restore_helpers(&self.home) {
                tracing::error!(%error, "could not put the helpers back");
            }
            return Err(error.into());
        }
        tracing::info!(from = VERSION, to = %latest.backend, supervised, "updating");
        let factory = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(RESTART_DELAY).await;
            if supervised {
                // The supervisor starts the new backend and watches it,
                // handing it the open vault's key.
                factory.vault.hand_over(false);
                crate::account::analytics::backend_stopped(&factory.home);
                std::process::exit(0);
            }
            // Nobody to watch over it: become the new backend in place, the
            // open vault's key handed over through an fd it keeps.
            let mut command = std::process::Command::new(&backend);
            command.args(std::env::args_os().skip(1));
            let handed = factory.vault.hand_over(true);
            if let Some(fd) = &handed {
                command.env(crate::vault::handoff::FD_ENV, std::os::fd::AsRawFd::as_raw_fd(fd).to_string());
            }
            crate::account::analytics::backend_stopped(&factory.home);
            let error = command.exec();
            drop(handed);
            crate::account::analytics::backend_started(&factory);
            tracing::error!(%error, backend = %backend.display(), "couldn't start the update");
        });
        Ok(())
    }
}

/// The release's app that replaces `installed`: the one with its name, or
/// else the one with its bundle id, so an app renamed where it's installed
/// (Claktory.app from the disk image, where releases have Factory.app)
/// still updates.
fn release_app(release: &Path, installed: &Path) -> Option<std::path::PathBuf> {
    let same_name = release.join(installed.file_name()?);
    if same_name.is_dir() {
        return Some(same_name);
    }
    let id = bundle_id(installed)?;
    let apps = std::fs::read_dir(release).ok()?.filter_map(Result::ok).map(|entry| entry.path());
    apps.filter(|path| path.extension().is_some_and(|extension| extension == "app") && path.is_dir())
        .find(|app| bundle_id(app).as_deref() == Some(id.as_str()))
}

/// The app's CFBundleIdentifier.
fn bundle_id(app: &Path) -> Option<String> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .ok()?;
    let id = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (output.status.success() && !id.is_empty()).then_some(id)
}

/// Whether this user may change what's in `folder` (/Applications isn't
/// for one who isn't an administrator).
fn writable(folder: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(folder.as_os_str().as_bytes()) else { return false };
    // SAFETY: a valid C string, for the duration of the call.
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

/// Writes an executable that has to survive a power cut: the bytes and
/// the rename reach the disk (`sync_all` is F_FULLFSYNC on macOS) before
/// it counts as written, so `bin/factoryd` is never left empty.
pub fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o755).open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent() {
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

async fn write_durably(path: std::path::PathBuf, bytes: Vec<u8>) -> std::io::Result<()> {
    tokio::task::spawn_blocking(move || write_executable(&path, &bytes)).await.map_err(std::io::Error::other)?
}

/// Puts `bin/factoryd.previous` back as `bin/factoryd`, durably, and the
/// helpers that went with it.
fn restore_previous(home: &Home) -> std::io::Result<()> {
    let previous = std::fs::read(home.previous_backend())?;
    write_executable(&home.backend(), &previous)?;
    restore_helpers(home)
}

/// The host that runs apps, in a release and in `<home>/bin`, beside
/// factoryd (where the backend looks for it).
pub const APPS_HOST: &str = "claktory-apps";

/// Speech recognition for voice messages, beside factoryd like the apps host.
pub const SPEECH_HOST: &str = "claktory-speech";

/// The programs a release carries beside factoryd, which updates put in
/// `<home>/bin` and roll back with it.
pub const HELPERS: [&str; 2] = [APPS_HOST, SPEECH_HOST];

fn helper(home: &Home, name: &str) -> std::path::PathBuf {
    home.root().join("bin").join(name)
}

fn previous_helper(home: &Home, name: &str) -> std::path::PathBuf {
    home.root().join("bin").join(format!("{name}.previous"))
}

/// Before an update's backend goes in: each helper the release carries
/// (`claktory-apps`, `claktory-speech`) replaces `bin/<name>`, which is
/// kept as `bin/<name>.previous` for a roll-back. A release without one
/// leaves what's there alone, and forgets an older `.previous` so a
/// roll-back doesn't put back a helper from before.
fn stage_helpers(home: &Home, release: &Path) -> std::io::Result<()> {
    for name in HELPERS {
        stage_helper(home, name, &release.join(name))?;
    }
    Ok(())
}

fn stage_helper(home: &Home, name: &str, new: &Path) -> std::io::Result<()> {
    let (current, previous) = (helper(home, name), previous_helper(home, name));
    if !new.is_file() {
        return remove(&previous);
    }
    match std::fs::read(&current) {
        Ok(bytes) => write_executable(&previous, &bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => remove(&previous)?,
        Err(error) => return Err(error),
    }
    write_executable(&current, &std::fs::read(new)?)
}

/// Puts each `bin/<name>.previous` back, if the update kept one; tries
/// them all before reporting the first failure.
fn restore_helpers(home: &Home) -> std::io::Result<()> {
    let mut failed = Ok(());
    for name in HELPERS {
        let restored = match std::fs::read(previous_helper(home, name)) {
            Ok(bytes) => write_executable(&helper(home, name), &bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
        if failed.is_ok() {
            failed = restored;
        }
    }
    failed
}

/// At start: each helper the running version's release carries that
/// `bin` lacks is copied in beside this backend. An update is made by the
/// backend before it, which puts in only the helpers it knows: v0.1.13,
/// which knew no speech host, updated to v0.1.14 without one. Only when
/// this backend runs from `bin`, as installed; one already there is left
/// alone (the update put it there, with its `.previous` for a roll-back).
pub fn heal_helpers(home: &Home) {
    let bin = home.root().join("bin");
    let running = std::env::current_exe().ok().and_then(|exe| exe.parent()?.canonicalize().ok());
    if running.is_none() || running != bin.canonicalize().ok() {
        return;
    }
    for name in heal(home, &home.releases().join(VERSION)) {
        tracing::warn!(helper = name, version = VERSION, "put in a helper the update to this version left out");
    }
}

/// The helpers `release` carries that `bin` lacks, copied in; their names.
fn heal(home: &Home, release: &Path) -> Vec<&'static str> {
    let mut healed = Vec::new();
    for name in HELPERS {
        let (new, current) = (release.join(name), helper(home, name));
        if !new.is_file() || current.exists() {
            continue;
        }
        match std::fs::read(&new).and_then(|bytes| write_executable(&current, &bytes)) {
            Ok(()) => healed.push(name),
            Err(error) => tracing::error!(%error, helper = name, "could not put in a helper the update left out"),
        }
    }
    healed
}

/// For the supervisor, when the backend keeps failing to start outside an
/// update's trial (a backend a power cut left broken, say): puts the
/// previous one back, once, when there is one that differs. True if it did.
pub fn fall_back(home: &Home, how_it_ended: &str) -> bool {
    let (Ok(current), Ok(previous)) = (std::fs::read(home.backend()), std::fs::read(home.previous_backend())) else { return false };
    if previous.is_empty() || current == previous {
        return false;
    }
    if let Err(error) = restore_previous(home) {
        tracing::error!(%error, "could not go back to the previous backend");
        return false;
    }
    tracing::warn!(reason = %how_it_ended, "the backend kept stopping; went back to the previous one");
    // Which version broke, the supervisor can't tell: it isn't the backend.
    let failed = Failed { version: String::new(), back_to: String::new(), reason: how_it_ended.to_owned() };
    if let Err(error) = write_json(&home.update_failed_path(), &failed) {
        tracing::error!(%error, "could not record why it went back");
    }
    true
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn bundle(at: &Path, marker: &str) {
        std::fs::create_dir_all(at.join("Contents")).expect("dir");
        let info = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>test.swap.dev</string></dict></plist>"#;
        std::fs::write(at.join("Contents/Info.plist"), info).expect("plist");
        std::fs::write(at.join("Contents/which"), marker).expect("marker");
    }

    #[test]
    fn the_app_is_swapped_with_the_old_one_moved_aside_until_the_new_one_is_in() {
        let dir = std::env::temp_dir().join(format!("factoryd-swap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (app, new) = (dir.join("Applications/Fake.app"), dir.join("release/Fake.app"));
        bundle(&app, "old");
        bundle(&new, "new");
        // `open` stands in, so nothing is launched.
        std::fs::create_dir_all(dir.join("stub")).expect("stub");
        let opened = dir.join("opened");
        std::fs::write(dir.join("stub/open"), format!("#!/bin/sh\necho \"$@\" > \"{}\"\n", opened.display())).expect("open");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("stub/open"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let path = format!("{}:{}", dir.join("stub").display(), std::env::var("PATH").unwrap_or_default());
        let status = std::process::Command::new("/bin/sh").args(["-c", SWAP_APP, "sh"]).arg(&app).arg(&new).env("PATH", path).status();
        assert!(status.expect("sh").success());
        for _ in 0..200 {
            if opened.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(std::fs::read_to_string(&opened).expect("reopened"), format!("-g {}\n", app.display()));
        assert_eq!(std::fs::read_to_string(app.join("Contents/which")).expect("swapped"), "new");
        let left: Vec<_> =
            std::fs::read_dir(dir.join("Applications")).expect("dir").map(|entry| entry.expect("entry").file_name()).collect();
        assert_eq!(left, ["Fake.app"], "nothing left beside it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn app_with_id(at: &Path, id: &str) {
        std::fs::create_dir_all(at.join("Contents")).expect("dir");
        let info = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>{id}</string></dict></plist>"#
        );
        std::fs::write(at.join("Contents/Info.plist"), info).expect("plist");
    }

    #[test]
    fn an_app_renamed_where_it_is_installed_is_updated_by_the_release_app_with_its_bundle_id() {
        let dir = std::env::temp_dir().join(format!("factoryd-release-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (installed, release) = (dir.join("Applications/Claktory.app"), dir.join("release"));
        app_with_id(&installed, "dev.factory.app");
        app_with_id(&release.join("Other.app"), "dev.other");
        app_with_id(&release.join("Factory.app"), "dev.factory.app");
        std::fs::create_dir_all(release.join("supervisor/Claktory Agents.app")).expect("supervisor");
        assert_eq!(release_app(&release, &installed), Some(release.join("Factory.app")));
        // The same name, as before, first.
        app_with_id(&release.join("Claktory.app"), "dev.factory.app");
        assert_eq!(release_app(&release, &installed), Some(release.join("Claktory.app")));
        // Nothing of its own in the release.
        let stranger = dir.join("Applications/Stranger.app");
        app_with_id(&stranger, "dev.stranger");
        assert_eq!(release_app(&release, &stranger), None);

        // A folder this user can't change is found out before anything moves.
        let applications = dir.join("Applications");
        assert!(writable(&applications));
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&applications, std::fs::Permissions::from_mode(0o555)).expect("chmod");
        let locked = writable(&applications);
        std::fs::set_permissions(&applications, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(!locked);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod helpers_tests {
    use super::*;

    fn home(name: &str) -> (Home, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("factoryd-helpers-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("home/bin")).expect("bin");
        std::fs::create_dir_all(dir.join("release")).expect("release");
        (Home::new(dir.join("home")), dir)
    }

    fn read(path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    /// An update made by v0.1.13's backend, which knew no speech host: the
    /// release has one, `bin` doesn't. The next start puts it in, and leaves
    /// the helpers that are there alone.
    #[test]
    fn a_helper_an_older_backends_update_left_out_goes_in_at_the_next_start() {
        let (home, dir) = home("heal");
        std::fs::write(home.backend(), "backend 2").expect("backend");
        std::fs::write(helper(&home, APPS_HOST), "host 1").expect("host");
        std::fs::write(dir.join("release/factoryd"), "backend 2").expect("release backend");
        std::fs::write(dir.join("release/claktory-apps"), "host 2").expect("release host");
        std::fs::write(dir.join("release/claktory-speech"), "speech 2").expect("release speech");
        assert_eq!(heal(&home, &dir.join("release")), [SPEECH_HOST]);
        assert_eq!(read(&helper(&home, SPEECH_HOST)).as_deref(), Some("speech 2"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(helper(&home, SPEECH_HOST)).expect("meta").permissions().mode() & 0o777, 0o755);
        assert_eq!(read(&helper(&home, APPS_HOST)).as_deref(), Some("host 1"), "left alone");
        assert!(!previous_helper(&home, SPEECH_HOST).exists(), "nothing to roll back to");
        // Once in, the next start has nothing to do; no release, nothing either.
        assert!(heal(&home, &dir.join("release")).is_empty());
        assert!(heal(&home, &dir.join("nowhere")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_update_replaces_the_helpers_and_a_roll_back_puts_them_back_with_the_backend() {
        let (home, dir) = home("replace");
        std::fs::write(helper(&home, APPS_HOST), "host 1").expect("host");
        std::fs::write(helper(&home, SPEECH_HOST), "speech 1").expect("speech");
        std::fs::write(home.backend(), "backend 2").expect("backend");
        std::fs::write(home.previous_backend(), "backend 1").expect("previous");
        std::fs::write(dir.join("release/claktory-apps"), "host 2").expect("new host");
        std::fs::write(dir.join("release/claktory-speech"), "speech 2").expect("new speech");

        stage_helpers(&home, &dir.join("release")).expect("staged");
        assert_eq!(read(&helper(&home, APPS_HOST)).as_deref(), Some("host 2"));
        assert_eq!(read(&helper(&home, SPEECH_HOST)).as_deref(), Some("speech 2"));
        assert_eq!(read(&previous_helper(&home, APPS_HOST)).as_deref(), Some("host 1"));
        assert_eq!(read(&previous_helper(&home, SPEECH_HOST)).as_deref(), Some("speech 1"));
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(helper(&home, APPS_HOST)).expect("meta").permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::metadata(helper(&home, SPEECH_HOST)).expect("meta").permissions().mode() & 0o777, 0o755);

        roll_back(&home, "It stopped.").expect("rolled back");
        assert_eq!(read(&home.backend()).as_deref(), Some("backend 1"));
        assert_eq!(read(&helper(&home, APPS_HOST)).as_deref(), Some("host 1"), "the host that went with it");
        assert_eq!(read(&helper(&home, SPEECH_HOST)).as_deref(), Some("speech 1"), "the speech host that went with it");

        // The supervisor's fall-back does the same.
        std::fs::write(home.backend(), "backend 2").expect("backend");
        std::fs::write(helper(&home, APPS_HOST), "host 2").expect("host");
        std::fs::write(helper(&home, SPEECH_HOST), "speech 2").expect("speech");
        assert!(fall_back(&home, "It stopped by itself."));
        assert_eq!(read(&helper(&home, APPS_HOST)).as_deref(), Some("host 1"));
        assert_eq!(read(&helper(&home, SPEECH_HOST)).as_deref(), Some("speech 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_first_helper_has_nothing_to_go_back_to_and_a_release_without_one_leaves_it_alone() {
        let (home, dir) = home("first");
        std::fs::write(home.previous_backend(), "backend 1").expect("previous");
        std::fs::write(dir.join("release/claktory-apps"), "host 1").expect("new host");
        // This release carries no claktory-speech, like ones from before it.
        stage_helpers(&home, &dir.join("release")).expect("staged");
        assert_eq!(read(&helper(&home, APPS_HOST)).as_deref(), Some("host 1"));
        assert!(!previous_helper(&home, APPS_HOST).exists());
        assert!(!helper(&home, SPEECH_HOST).exists());
        restore_previous(&home).expect("restored");
        assert_eq!(read(&helper(&home, APPS_HOST)).as_deref(), Some("host 1"), "nothing kept to put back");

        // A release without a helper: the installed one stays, and an old
        // `.previous` is forgotten so a roll-back can't bring it back.
        std::fs::write(helper(&home, SPEECH_HOST), "speech 1").expect("speech");
        std::fs::write(previous_helper(&home, APPS_HOST), "host 0").expect("stale");
        std::fs::write(previous_helper(&home, SPEECH_HOST), "speech 0").expect("stale");
        std::fs::remove_file(dir.join("release/claktory-apps")).expect("removed");
        stage_helpers(&home, &dir.join("release")).expect("staged");
        assert_eq!(read(&helper(&home, APPS_HOST)).as_deref(), Some("host 1"));
        assert_eq!(read(&helper(&home, SPEECH_HOST)).as_deref(), Some("speech 1"));
        assert!(!previous_helper(&home, APPS_HOST).exists());
        assert!(!previous_helper(&home, SPEECH_HOST).exists());
        restore_previous(&home).expect("restored");
        assert_eq!(read(&helper(&home, APPS_HOST)).as_deref(), Some("host 1"));
        assert_eq!(read(&helper(&home, SPEECH_HOST)).as_deref(), Some("speech 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
