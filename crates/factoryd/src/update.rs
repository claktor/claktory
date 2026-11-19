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

use crate::factory::{Factory, RequestError, write_atomic};
use crate::home::Home;

/// This build's version, stamped by the build scripts; "dev" otherwise.
pub const VERSION: &str = match option_env!("FACTORY_VERSION") {
    Some(version) => version,
    None => "dev",
};

/// Set by the supervisor for the backend it runs.
pub const SUPERVISED: &str = "FACTORY_SUPERVISED";

/// Time for the ack to reach the app before the backend hands over.
const RESTART_DELAY: Duration = Duration::from_millis(300);

/// How long a new backend has to serve an app that's trying to connect.
const TRIAL: Duration = Duration::from_secs(30);

/// Waits for the app to quit (forcing it after 10s), swaps the bundle, and
/// reopens it; Factory Dev in the background, as always.
const SWAP_APP: &str = r#"app="$1"; new="$2"
(
  for _ in $(seq 100); do pgrep -f "$app/Contents/MacOS/" >/dev/null || break; sleep 0.1; done
  pkill -f "$app/Contents/MacOS/"
  rm -rf "$app.updating" && ditto "$new" "$app.updating" && rm -rf "$app" && mv "$app.updating" "$app" || exit 1
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

/// For the supervisor, after the backend stopped or couldn't be started:
/// whether that was an update failing. A trial that hasn't started yet means
/// the backend stopped to hand over to the update.
pub fn update_failed(home: &Home) -> bool {
    read_trial(home).is_some_and(|trial| trial.started)
}

/// Puts the previous backend back after a failed update, and records why.
pub fn roll_back(home: &Home, how_it_ended: &str) -> std::io::Result<()> {
    let trial = read_trial(home);
    let previous = home.previous_backend();
    let tmp = home.backend().with_extension("restoring");
    std::fs::copy(&previous, &tmp)?;
    std::fs::rename(&tmp, home.backend())?;
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
        Ok(failed) => {
            format!("Factory {} didn't start, so Factory went back to {}. {}", failed.version, failed.back_to, failed.reason)
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
            let new_app = match app.file_name() {
                Some(name) if app.extension().is_some_and(|extension| extension == "app") && app.is_dir() => release.join(name),
                _ => return Err(RequestError::NotAnApp(app_path.to_owned())),
            };
            if !new_app.is_dir() {
                return Err(RequestError::NotAnApp(app_path.to_owned()));
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
        if supervised {
            let running = tokio::fs::read(std::env::current_exe()?).await?;
            write_atomic(&self.home.previous_backend(), &running).await?;
            set_executable(&self.home.previous_backend()).await?;
            let trial = Trial { from: VERSION.to_owned(), to: latest.backend.clone(), started: false, reason: None };
            write_json(&self.home.trial_path(), &trial)?;
        }
        write_atomic(&backend, &new_backend).await?;
        set_executable(&backend).await?;
        tracing::info!(from = VERSION, to = %latest.backend, supervised, "updating");
        tokio::spawn(async move {
            tokio::time::sleep(RESTART_DELAY).await;
            if supervised {
                // The supervisor starts the new backend and watches it.
                std::process::exit(0);
            }
            // Nobody to watch over it: become the new backend in place.
            let error = std::process::Command::new(&backend).args(std::env::args_os().skip(1)).exec();
            tracing::error!(%error, backend = %backend.display(), "couldn't start the update");
        });
        Ok(())
    }
}

async fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).await
}
