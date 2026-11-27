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

#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use factory_protocol::v1::{self, UpdateCheckState as CheckState};

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
#[cfg(unix)]
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
    if !plain(version) || !dir.join(crate::home::program("factoryd")).is_file() {
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

/// The backend a rollback would go back to: the one before the update on
/// trial, if one is.
pub(crate) fn rollback_version(home: &Home) -> Option<String> {
    read_trial(home).map(|trial| trial.from).filter(|from| !from.is_empty())
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

/// Asking again within this of the last check gives its answer again.
const CHECK_AGAIN_AFTER: Duration = Duration::from_secs(10);

/// The release channel, shared by its schedule and Check for updates so
/// only one of them reads or downloads at a time, and the person's checks.
#[derive(Default)]
pub struct Updates {
    channel: std::sync::OnceLock<Arc<tokio::sync::Mutex<crate::channel::Channel>>>,
    /// A check the person asked for is running.
    asked: AtomicBool,
    /// When the person's last check finished.
    last: std::sync::Mutex<Option<std::time::Instant>>,
}

/// What a finished check says: `failed`, unless a newer release than this
/// backend is here to offer.
fn answer(latest: &Release, failed: Option<String>) -> v1::UpdateCheck {
    answer_for(VERSION, latest, failed)
}

/// As `answer`, for a backend of version `running`. An unstamped one ("dev")
/// is older than any release, as the channel takes it.
fn answer_for(running: &str, latest: &Release, failed: Option<String>) -> v1::UpdateCheck {
    let checked_ms = crate::transcript::now_ms();
    let offered = !latest.version.is_empty() && (running == "dev" || newer(&latest.version, running));
    if offered {
        return v1::UpdateCheck { state: CheckState::Ready.into(), version: latest.version.clone(), checked_ms, ..Default::default() };
    }
    if let Some(error) = failed {
        return v1::UpdateCheck { state: CheckState::Failed.into(), error, checked_ms, ..Default::default() };
    }
    v1::UpdateCheck { state: CheckState::UpToDate.into(), version: running.to_owned(), checked_ms, ..Default::default() }
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
        // No rollback now: conversations are saved as JSON Lines.
        self.old_segments.store(false, std::sync::atomic::Ordering::Relaxed);
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
            // A release that turned up after a check, from anywhere, is
            // what that check's answer says now.
            if matches!(inner.update_check.state(), CheckState::UpToDate | CheckState::Ready | CheckState::Failed) {
                inner.update_check = answer(&inner.latest_release, None);
            }
            self.broadcast_workspace(&inner);
        }
    }

    /// Settings' Check for updates: checks the channel and the releases
    /// here now, in the background; Workspace.update_check says what it
    /// found. While a check runs, or within `CHECK_AGAIN_AFTER` of the last,
    /// asking again changes nothing.
    pub fn check_for_updates(self: &Arc<Self>) -> Result<(), RequestError> {
        {
            let mut last = self.updates.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.updates.asked.load(Ordering::Relaxed) || last.is_some_and(|at| at.elapsed() < CHECK_AGAIN_AFTER) {
                return Ok(());
            }
            self.updates.asked.store(true, Ordering::Relaxed);
            *last = None;
        }
        self.show_check(v1::UpdateCheck { state: CheckState::Checking.into(), checked_ms: crate::transcript::now_ms(), ..Default::default() });
        let factory = self.clone();
        tokio::spawn(async move {
            let failed = match factory.updates.channel.get().cloned() {
                // Waits for a check of the schedule's that's running.
                Some(channel) => factory.check_channel(&mut *channel.lock().await, true).await.err(),
                None => None,
            };
            factory.refresh_release().await;
            {
                let mut inner = factory.lock();
                inner.update_check = answer(&inner.latest_release, failed);
                factory.broadcast_workspace(&inner);
            }
            *factory.updates.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(std::time::Instant::now());
            factory.updates.asked.store(false, Ordering::Relaxed);
        });
        Ok(())
    }

    /// Checks the release channel at `url` from now on (`channel::poll`
    /// checks it on a schedule); once, at start.
    pub fn use_channel(&self, url: &str) {
        let _ = self.updates.channel.set(Arc::new(tokio::sync::Mutex::new(crate::channel::Channel::new(self.home.clone(), url))));
    }

    /// The scheduled check of the channel. It keeps an answer the person
    /// saw current, but shows a failure only of a download it showed; any
    /// other failure leaves the answer as it was.
    pub async fn check_on_schedule(self: &Arc<Self>) {
        let Some(channel) = self.updates.channel.get().cloned() else { return };
        let result = self.check_channel(&mut *channel.lock().await, false).await;
        self.refresh_release().await;
        if self.updates.asked.load(Ordering::Relaxed) {
            return;
        }
        let mut inner = self.lock();
        let shown = inner.update_check.state();
        if shown == CheckState::None {
            return;
        }
        let now = match result {
            Ok(()) => answer(&inner.latest_release, None),
            // The download it showed failed.
            Err(why) if shown == CheckState::Downloading => answer(&inner.latest_release, Some(why)),
            // A failure nobody asked about leaves the answer as it was: it
            // never turns into "up to date".
            Err(_) => return,
        };
        inner.update_check = now;
        self.broadcast_workspace(&inner);
    }

    /// Reads the channel and downloads a newer release, showing it as
    /// downloading when the person asked or saw an answer already. Err says
    /// why it couldn't, for the person.
    async fn check_channel(&self, channel: &mut crate::channel::Channel, asked: bool) -> Result<(), String> {
        use crate::channel::{ChannelError, Checked, Found};
        let words = |error: ChannelError| {
            tracing::warn!(%error, "could not check for releases");
            match error {
                ChannelError::Fetch(_) => "Claktory couldn't reach the update server. Check your internet connection, then try again.".to_owned(),
                ChannelError::Manifest(_) => "The update server's answer couldn't be read. Try again later.".to_owned(),
                ChannelError::Unsigned(_) => "The update offered isn't signed by Claktory, so it wasn't downloaded.".to_owned(),
                ChannelError::Rejected { version, .. } => format!("{version} didn't download correctly, so Claktory didn't keep it."),
                ChannelError::Stale(_) => "Couldn't check for updates: the answer was older than the last one. Try again later.".to_owned(),
                ChannelError::Io(error) => format!("Claktory couldn't save the update: {error}."),
            }
        };
        match channel.find().await {
            Ok(Found::UpToDate) => Ok(()),
            Ok(Found::Skipped { version, why }) => Err(format!("{version} is out, but it can't run here: {why}.")),
            Ok(Found::Newer(manifest, tarball)) => {
                if asked || self.lock().update_check.state() != CheckState::None {
                    self.show_check(v1::UpdateCheck {
                        state: CheckState::Downloading.into(),
                        version: manifest.version.clone(),
                        checked_ms: crate::transcript::now_ms(),
                        ..Default::default()
                    });
                }
                match channel.fetch(&manifest, &tarball).await {
                    Ok(Checked::Downloaded(version)) => {
                        tracing::info!(%version, "release ready to install");
                        Ok(())
                    }
                    Ok(_) => Ok(()),
                    Err(error) => Err(words(error)),
                }
            }
            Err(error) => Err(words(error)),
        }
    }

    fn show_check(&self, check: v1::UpdateCheck) {
        let mut inner = self.lock();
        inner.update_check = check;
        self.broadcast_workspace(&inner);
    }

    pub async fn install_update(self: &Arc<Self>, app_path: &str) -> Result<(), RequestError> {
        // Never while Claktory stops (friend bug #27), and no stop while it
        // goes in: the backend's going away would read as a failed update.
        self.service.begin_update()?;
        let installed = self.put_update_in(app_path).await;
        if !matches!(installed, Ok(true)) {
            self.service.update_failed();
        }
        installed.map(drop)
    }

    /// Installs the update; Ok(true) when this backend stops for the new one.
    async fn put_update_in(self: &Arc<Self>, app_path: &str) -> Result<bool, RequestError> {
        let latest = self.lock().latest_release.clone();
        if latest.version.is_empty() {
            return Err(RequestError::NoRelease);
        }
        let release = self.home.releases().join(&latest.version);
        // On Windows the app names the folder it's installed in, which must
        // be Claktory's own: the new version goes in beside the one running,
        // and the launcher starts it once the app restarts.
        #[cfg(windows)]
        if !app_path.is_empty() {
            let root = crate::windows::dirs::programs()?;
            let same = |path: &Path| crate::windows::fs::canonical(path).ok().map(|path| path.to_string_lossy().to_lowercase());
            if same(Path::new(app_path)).is_none() || same(Path::new(app_path)) != same(&root) {
                return Err(RequestError::NotAnApp(app_path.to_owned()));
            }
            let (from, version) = (release.clone(), latest.version.clone());
            tokio::task::spawn_blocking(move || {
                crate::windows::app_install::install(
                    &root,
                    &from,
                    &version,
                    crate::windows::authenticode::check_release,
                    crate::windows::authenticode::check_program,
                )
            })
            .await
            .map_err(std::io::Error::other)?
            .map_err(|why| std::io::Error::other(format!("Claktory couldn't put the new app in: {why}")))?;
        }
        #[cfg(unix)]
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
            return Ok(false);
        }
        let backend = self.home.backend();
        let new_backend = tokio::fs::read(release.join(crate::home::program("factoryd"))).await?;
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
                #[cfg(unix)]
                factory.vault.hand_over(false);
                #[cfg(windows)]
                factory.vault.hand_over();
                factory.store.write_calls();
                crate::account::analytics::backend_stopped(&factory.home);
                std::process::exit(0);
            }
            // Nobody to watch over it, on Windows, where nothing replaces a
            // process in place: the new backend is started, and this one
            // exits once it has. The vault opens again by itself, or asks.
            #[cfg(windows)]
            {
                factory.vault.hand_over();
                factory.store.write_calls();
                crate::account::analytics::backend_stopped(&factory.home);
                match std::process::Command::new(&backend).args(std::env::args_os().skip(1)).spawn() {
                    Ok(_) => std::process::exit(0),
                    Err(error) => {
                        crate::account::analytics::backend_started(&factory);
                        tracing::error!(%error, backend = %backend.display(), "couldn't start the update");
                    }
                }
            }
            // Nobody to watch over it: become the new backend in place, the
            // open vault's key handed over through an fd it keeps.
            #[cfg(unix)]
            {
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
            }
        });
        Ok(true)
    }
}

/// The release's app that replaces `installed`: the one with its name, or
/// else the one with its bundle id, so an app renamed where it's installed
/// (Claktory.app from the disk image, where releases have Factory.app)
/// still updates.
#[cfg(unix)]
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
#[cfg(unix)]
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
#[cfg(unix)]
fn writable(folder: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(folder.as_os_str().as_bytes()) else { return false };
    // SAFETY: a valid C string, for the duration of the call.
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

/// Writes an executable that has to survive a power cut: the bytes and
/// the rename reach the disk (`sync_all` is F_FULLFSYNC on macOS) before
/// it counts as written, so `bin/factoryd` is never left empty.
#[cfg(unix)]
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

/// On Windows, where a running program's file can't be written over but can
/// be renamed: the new one is written beside it and flushed, the one there
/// is moved aside (`<name>.replaced-<random>`), and the new one renamed into
/// its place, written through to the disk. What's moved aside is deleted
/// when nothing runs it, now or by `sweep_replaced` later.
#[cfg(windows)]
pub fn write_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    let aside = crate::windows::fs::replace(&tmp, path)?;
    if let Some(aside) = aside {
        let _ = std::fs::remove_file(aside);
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

/// This backend's program, links resolved: macOS gives the path it was
/// started by, so `factoryd` run through the installer's link on PATH
/// (`~/.local/bin/factoryd`) would look for its helpers beside the link
/// (friend bug #24). The path as given when it can't be resolved.
pub fn running_exe() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.canonicalize().unwrap_or(exe))
}

/// `name` (one of `HELPERS`) beside this backend's program, if it's there.
pub(crate) fn beside_backend(name: &str) -> Option<std::path::PathBuf> {
    let beside = running_exe()?.parent()?.join(crate::home::program(name));
    beside.is_file().then_some(beside)
}

/// The host that runs apps, in a release and in `<home>/bin`, beside
/// factoryd (where the backend looks for it).
pub const APPS_HOST: &str = "claktory-apps";

/// Speech recognition for voice messages, beside factoryd like the apps host.
pub const SPEECH_HOST: &str = "claktory-speech";

/// Phone-sized video previews (#379, `previews/video.rs`), on a Mac,
/// beside factoryd like the apps host.
pub const VIDEO_PREVIEW: &str = "claktory-video-preview";

/// The WSL helper (#490, `scripts/claktory-wsl.sh`), in Linux releases from
/// v0.1.23 on, beside factoryd like the apps host. Mac releases have none.
pub const WSL_HELPER: &str = "claktory-wsl";

/// The programs a release carries beside factoryd, which updates put in
/// `<home>/bin` and roll back with it. A release may lack any of them (one
/// from before it, or for another system): what's installed then stays as
/// it is, and nothing is said.
pub const HELPERS: [&str; 4] = [APPS_HOST, SPEECH_HOST, VIDEO_PREVIEW, WSL_HELPER];

fn helper(home: &Home, name: &str) -> std::path::PathBuf {
    home.root().join("bin").join(crate::home::program(name))
}

fn previous_helper(home: &Home, name: &str) -> std::path::PathBuf {
    home.root().join("bin").join(format!("{name}.previous"))
}

/// The programs a release carries for every agent, in its `tools/`, which
/// updates put in `<home>/tools` (first on agents' PATH, `tools_dir`) and
/// roll back with the backend, as they do the helpers.
pub const TOOLS: [&str; 2] = ["rg", "jq"];

/// `<home>/tools`: the programs Claktory gives every agent (`TOOLS`),
/// first on their PATH. Only these: the backend and its helpers stay in
/// `bin`, off it.
pub fn tools_dir(home: &Home) -> std::path::PathBuf {
    home.root().join("tools")
}

/// The PATH agents' commands run with: `<home>/tools` first, so its `rg`
/// is the one they get, then the PATH Claktory runs with. It's there even
/// before `<home>/tools` is (a test, a Claktory not installed from a
/// release, one an update is about to heal): a folder that isn't there is
/// passed over, and an agent's tools are set when its context starts.
pub fn agents_path(home: &Home) -> String {
    let tools = tools_dir(home);
    let inherited = std::env::var_os("PATH");
    let rest: Vec<std::path::PathBuf> = match &inherited {
        // An empty entry would mean the current folder.
        Some(path) => std::env::split_paths(path).filter(|dir| !dir.as_os_str().is_empty() && *dir != tools).collect(),
        // bash's own default, which setting PATH would otherwise lose.
        None => ["/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(std::path::PathBuf::from).to_vec(),
    };
    let joined = std::env::join_paths(std::iter::once(tools).chain(rest)).unwrap_or_default();
    joined.to_string_lossy().into_owned()
}

/// A program a release carries beside the backend: its name, where it is in
/// the release, where it goes, and where the one it replaced is kept for a
/// roll-back (for a tool, in `tools/.previous`, off agents' PATH).
struct Carried {
    name: &'static str,
    new: std::path::PathBuf,
    current: std::path::PathBuf,
    previous: std::path::PathBuf,
}

/// The helpers (`HELPERS`, in `bin`) and the tools (`TOOLS`, in `<home>/tools`).
fn carried(home: &Home, release: &Path) -> Vec<Carried> {
    let helpers = HELPERS.into_iter().map(|name| Carried {
        name,
        new: release.join(crate::home::program(name)),
        current: helper(home, name),
        previous: previous_helper(home, name),
    });
    // On Windows in `tools\bin`, beside the Git folders.
    let tool = |name: &str| {
        if cfg!(windows) { Path::new("bin").join(crate::home::program(name)) } else { std::path::PathBuf::from(name) }
    };
    let tools = TOOLS.into_iter().map(|name| Carried {
        name,
        new: release.join("tools").join(tool(name)),
        current: tools_dir(home).join(tool(name)),
        previous: tools_dir(home).join(".previous").join(tool(name)),
    });
    helpers.chain(tools).collect()
}

/// Puts the tools' licenses (`LICENSES`, ripgrep's among them) from a
/// release or the app beside them in `<home>/tools/LICENSES`, as they came,
/// replacing those there. Text, not programs: nothing to roll back.
pub fn put_tool_licenses(from: &Path, home: &Home) -> std::io::Result<()> {
    fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                copy(&entry.path(), &to.join(entry.file_name()))?;
            } else if kind.is_file() {
                std::fs::copy(entry.path(), to.join(entry.file_name()))?;
            }
        }
        Ok(())
    }
    if !from.is_dir() {
        return Ok(());
    }
    let to = tools_dir(home).join("LICENSES");
    let staged = tools_dir(home).join(".LICENSES.new");
    let _ = std::fs::remove_dir_all(&staged);
    copy(from, &staged)?;
    let _ = std::fs::remove_dir_all(&to);
    std::fs::rename(&staged, &to)
}

/// Writes an executable, making its folder first (`<home>/tools` may not be
/// there yet).
fn put_executable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    write_executable(path, bytes)
}

/// Before an update's backend goes in: each helper and tool the release
/// carries (`claktory-apps`, `claktory-speech`, `claktory-video-preview`,
/// `claktory-wsl`; `tools/rg`, `tools/jq`) replaces the installed one, which is kept (`bin/<name>.previous`,
/// `tools/.previous/<name>`) for a roll-back. A release without one leaves
/// what's there alone, and forgets an older previous copy so a roll-back
/// doesn't put back one from before.
fn stage_helpers(home: &Home, release: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    crate::windows::tools::stage(home, release)?;
    for carried in carried(home, release) {
        stage_helper(&carried)?;
    }
    if let Err(error) = put_tool_licenses(&release.join("tools/LICENSES"), home) {
        tracing::warn!(%error, "could not put in the tools' licenses");
    }
    Ok(())
}

fn stage_helper(carried: &Carried) -> std::io::Result<()> {
    let Carried { new, current, previous, .. } = carried;
    if !new.is_file() {
        return remove(previous);
    }
    match std::fs::read(current) {
        Ok(bytes) => put_executable(previous, &bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => remove(previous)?,
        Err(error) => return Err(error),
    }
    put_executable(current, &std::fs::read(new)?)
}

/// Puts each previous copy back, if the update kept one; tries them all
/// before reporting the first failure.
fn restore_helpers(home: &Home) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut failed = crate::windows::tools::restore(home);
    #[cfg(unix)]
    let mut failed = Ok(());
    for Carried { current, previous, .. } in carried(home, Path::new("")) {
        let restored = match std::fs::read(&previous) {
            Ok(bytes) => write_executable(&current, &bytes),
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
/// `bin` lacks is copied in beside this backend, and each tool `<home>/tools`
/// lacks into it. An update is made by the backend before it, which puts in
/// only the helpers it knows: v0.1.13, which knew no speech host, updated to
/// v0.1.14 without one, and v0.1.20 knew no tools. Only when
/// this backend runs from `bin`, as installed; one already there is left
/// alone (the update put it there, with its `.previous` for a roll-back).
pub fn heal_helpers(home: &Home) {
    let bin = home.root().join("bin");
    let running = running_exe().and_then(|exe| exe.parent()?.canonicalize().ok());
    if running.is_none() || running != bin.canonicalize().ok() {
        return;
    }
    for name in heal(home, &home.releases().join(VERSION)) {
        tracing::warn!(helper = name, version = VERSION, "put in a helper the update to this version left out");
    }
    #[cfg(windows)]
    crate::windows::tools::heal_and_sweep(home, &home.releases().join(VERSION));
}

/// The helpers and tools `release` carries that aren't installed, copied
/// in; their names.
fn heal(home: &Home, release: &Path) -> Vec<&'static str> {
    let mut healed = Vec::new();
    for Carried { name, new, current, .. } in carried(home, release) {
        if !new.is_file() || current.exists() {
            continue;
        }
        match std::fs::read(&new).and_then(|bytes| put_executable(&current, &bytes)) {
            Ok(()) => healed.push(name),
            Err(error) => tracing::error!(%error, helper = name, "could not put in a helper the update left out"),
        }
    }
    // With a tool put in (jq, after an update from a backend that knew
    // only rg), its license comes too.
    let tool_healed = healed.iter().any(|name| TOOLS.contains(name));
    if (tool_healed || !tools_dir(home).join("LICENSES").exists())
        && let Err(error) = put_tool_licenses(&release.join("tools/LICENSES"), home)
    {
        tracing::warn!(%error, "could not put in the tools' licenses");
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

    #[test]
    fn a_check_offers_a_newer_release_even_when_the_channel_failed() {
        let release = |version: &str| Release { version: version.into(), backend: version.into(), changelog: Vec::new() };
        let failed = || Some("no network".to_owned());
        let ready = answer_for("v0.1.16", &release("v0.1.17"), failed());
        assert_eq!((ready.state(), ready.version.as_str(), ready.error.as_str()), (CheckState::Ready, "v0.1.17", ""));
        let current = answer_for("v0.1.16", &release("v0.1.16"), None);
        assert_eq!((current.state(), current.version.as_str()), (CheckState::UpToDate, "v0.1.16"));
        let older = answer_for("v0.1.16", &release("v0.1.15"), failed());
        assert_eq!((older.state(), older.error.as_str()), (CheckState::Failed, "no network"));
        assert_eq!(answer_for("v0.1.16", &release(""), None).state(), CheckState::UpToDate);
        assert_eq!(answer_for("dev", &release("v0.1.3-dev.4"), None).state(), CheckState::Ready);
    }

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
        // `open` stands in, so nothing is launched. It's the swap's last
        // step, and what it says appears all at once (written aside, then
        // renamed), so its file there means the swap is done, whatever the
        // machine's load.
        std::fs::create_dir_all(dir.join("stub")).expect("stub");
        let opened = dir.join("opened");
        let stub = format!("#!/bin/sh\necho \"$@\" > \"{0}.part\" && mv \"{0}.part\" \"{0}\"\n", opened.display());
        std::fs::write(dir.join("stub/open"), stub).expect("open");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("stub/open"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let path = format!("{}:{}", dir.join("stub").display(), std::env::var("PATH").unwrap_or_default());
        let status = std::process::Command::new("/bin/sh").args(["-c", SWAP_APP, "sh"]).arg(&app).arg(&new).env("PATH", path).status();
        assert!(status.expect("sh").success());
        // A failsafe only, so a broken swap fails rather than hangs.
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        while !opened.exists() && std::time::Instant::now() < deadline {
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

    /// A release's or home's file as this system names it: `.exe` on
    /// Windows, and the tools in `tools\bin` there.
    fn file(path: &str) -> std::path::PathBuf {
        if !cfg!(windows) {
            return path.into();
        }
        let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
        let tool = TOOLS.contains(&name);
        let mut file = Path::new(folder).to_path_buf();
        if tool {
            file.push("bin");
        }
        file.push(if tool || HELPERS.contains(&name) || name == "factoryd" { crate::home::program(name) } else { name.to_owned() });
        file
    }

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

    /// A Linux release from v0.1.23 on carries the WSL helper, which an
    /// update by v0.1.22, knowing none, leaves out: the next start puts it
    /// in. A Mac release has none: nothing is put in or said.
    #[test]
    fn the_wsl_helper_goes_in_where_a_release_carries_it_and_nothing_happens_where_it_doesnt() {
        let (home, dir) = home("wsl");
        std::fs::write(dir.join(file("release/factoryd")), "backend 2").expect("release backend");
        std::fs::write(dir.join(file("release/claktory-apps")), "host 2").expect("release host");
        std::fs::write(helper(&home, APPS_HOST), "host 2").expect("host");
        assert!(heal(&home, &dir.join("release")).is_empty(), "a Mac release, with no WSL helper");
        assert!(!helper(&home, WSL_HELPER).exists());
        stage_helpers(&home, &dir.join("release")).expect("staged");
        assert!(!helper(&home, WSL_HELPER).exists() && !previous_helper(&home, WSL_HELPER).exists());

        std::fs::write(dir.join(file("release/claktory-wsl")), "wsl 1").expect("release wsl");
        assert_eq!(heal(&home, &dir.join("release")), [WSL_HELPER]);
        assert_eq!(read(&helper(&home, WSL_HELPER)).as_deref(), Some("wsl 1"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(helper(&home, WSL_HELPER)).expect("meta").permissions().mode() & 0o777, 0o755);
        }

        // The next update replaces it, keeping this one for a roll-back.
        std::fs::write(home.previous_backend(), "backend 1").expect("previous");
        std::fs::write(dir.join(file("release/claktory-wsl")), "wsl 2").expect("release wsl");
        stage_helpers(&home, &dir.join("release")).expect("staged");
        assert_eq!(read(&helper(&home, WSL_HELPER)).as_deref(), Some("wsl 2"));
        assert_eq!(read(&previous_helper(&home, WSL_HELPER)).as_deref(), Some("wsl 1"));
        restore_helpers(&home).expect("restored");
        assert_eq!(read(&helper(&home, WSL_HELPER)).as_deref(), Some("wsl 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An update made by v0.1.13's backend, which knew no speech host: the
    /// release has one, `bin` doesn't. The next start puts it in, and leaves
    /// the helpers that are there alone.
    #[test]
    fn a_helper_an_older_backends_update_left_out_goes_in_at_the_next_start() {
        let (home, dir) = home("heal");
        std::fs::write(home.backend(), "backend 2").expect("backend");
        std::fs::write(helper(&home, APPS_HOST), "host 1").expect("host");
        std::fs::write(dir.join(file("release/factoryd")), "backend 2").expect("release backend");
        std::fs::write(dir.join(file("release/claktory-apps")), "host 2").expect("release host");
        std::fs::write(dir.join(file("release/claktory-speech")), "speech 2").expect("release speech");
        assert_eq!(heal(&home, &dir.join("release")), [SPEECH_HOST]);
        assert_eq!(read(&helper(&home, SPEECH_HOST)).as_deref(), Some("speech 2"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(helper(&home, SPEECH_HOST)).expect("meta").permissions().mode() & 0o777, 0o755);
        }
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
        std::fs::write(dir.join(file("release/claktory-apps")), "host 2").expect("new host");
        std::fs::write(dir.join(file("release/claktory-speech")), "speech 2").expect("new speech");

        stage_helpers(&home, &dir.join("release")).expect("staged");
        assert_eq!(read(&helper(&home, APPS_HOST)).as_deref(), Some("host 2"));
        assert_eq!(read(&helper(&home, SPEECH_HOST)).as_deref(), Some("speech 2"));
        assert_eq!(read(&previous_helper(&home, APPS_HOST)).as_deref(), Some("host 1"));
        assert_eq!(read(&previous_helper(&home, SPEECH_HOST)).as_deref(), Some("speech 1"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(helper(&home, APPS_HOST)).expect("meta").permissions().mode() & 0o777, 0o755);
            assert_eq!(std::fs::metadata(helper(&home, SPEECH_HOST)).expect("meta").permissions().mode() & 0o777, 0o755);
        }

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
        std::fs::write(dir.join(file("release/claktory-apps")), "host 1").expect("new host");
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
        std::fs::remove_file(dir.join(file("release/claktory-apps"))).expect("removed");
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

    /// ripgrep and jq, for every agent: an update puts the release's tools in
    /// `<home>/tools` (made if need be), keeping the one it replaced off the
    /// agents' PATH; a roll-back puts that back; a start after an update
    /// from a backend that knew no tools heals it in.
    #[test]
    fn an_update_puts_the_agents_tools_in_and_a_roll_back_takes_them_back() {
        let (home, dir) = home("tools");
        let rg = tools_dir(&home).join(file("rg"));
        std::fs::write(home.backend(), "backend 2").expect("backend");
        std::fs::write(home.previous_backend(), "backend 1").expect("previous");
        std::fs::create_dir_all(dir.join("release/tools/bin")).expect("release tools");
        std::fs::write(dir.join(file("release/tools/rg")), "rg 15").expect("release rg");
        std::fs::write(dir.join(file("release/tools/jq")), "jq 1.8.1").expect("release jq");
        std::fs::create_dir_all(dir.join("release/tools/LICENSES/ripgrep")).expect("licenses");
        std::fs::write(dir.join("release/tools/LICENSES/ripgrep/LICENSE-MIT"), "MIT").expect("license");
        assert_eq!(heal(&home, &dir.join("release")), ["rg", "jq"], "a start after v0.1.20's update");
        assert_eq!(read(&tools_dir(&home).join("LICENSES/ripgrep/LICENSE-MIT")).as_deref(), Some("MIT"), "its license beside it");
        assert_eq!(read(&rg).as_deref(), Some("rg 15"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&rg).expect("meta").permissions().mode() & 0o777, 0o755);
        }

        std::fs::write(dir.join(file("release/tools/rg")), "rg 16").expect("newer rg");
        std::fs::write(dir.join(file("release/tools/jq")), "jq 1.8.2").expect("newer jq");
        stage_helpers(&home, &dir.join("release")).expect("staged");
        assert_eq!(read(&rg).as_deref(), Some("rg 16"));
        // On Windows the tools are in `tools\bin`, beside the Git folders.
        let on_path = if cfg!(windows) { tools_dir(&home).join("bin") } else { tools_dir(&home) };
        let kept: Vec<String> =
            std::fs::read_dir(on_path).expect("tools").filter_map(|entry| entry.ok()?.file_name().into_string().ok()).collect();
        let mut programs: Vec<&String> = kept.iter().filter(|name| !name.starts_with('.') && *name != "LICENSES").collect();
        programs.sort();
        let names: Vec<String> = ["jq", "rg"].map(crate::home::program).to_vec();
        assert_eq!(programs, names.iter().collect::<Vec<_>>(), "only the tools are on the PATH: {kept:?}");
        roll_back(&home, "It stopped.").expect("rolled back");
        assert_eq!(read(&rg).as_deref(), Some("rg 15"), "the rg that went with the backend");
        assert_eq!(read(&tools_dir(&home).join(file("jq"))).as_deref(), Some("jq 1.8.1"), "and its jq");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// jq after rg (Pericolo 29760): a release without tools/jq (v0.1.21, or
    /// an older one) is passed over, nothing failing; one with it, started
    /// after an update by a backend that knew only rg, heals jq in with its
    /// license beside ripgrep's.
    #[test]
    fn a_tool_a_release_lacks_is_passed_over_and_a_new_one_comes_with_its_license() {
        let (home, dir) = home("jq");
        let release = dir.join("release");
        std::fs::create_dir_all(release.join("tools/LICENSES/ripgrep")).expect("release tools");
        std::fs::create_dir_all(release.join("tools/bin")).expect("release tools");
        std::fs::write(release.join(file("tools/rg")), "rg 15").expect("release rg");
        std::fs::write(release.join("tools/LICENSES/ripgrep/UNLICENSE"), "ripgrep's").expect("license");
        assert_eq!(heal(&home, &release), ["rg"], "no jq in v0.1.21: passed over");
        stage_helpers(&home, &release).expect("an update without jq stages");
        assert!(!tools_dir(&home).join(file("jq")).exists());

        std::fs::create_dir_all(release.join("tools/LICENSES/jq")).expect("jq licenses");
        std::fs::write(release.join(file("tools/jq")), "jq 1.8.2").expect("release jq");
        std::fs::write(release.join("tools/LICENSES/jq/COPYING"), "jq's").expect("license");
        assert_eq!(heal(&home, &release), ["jq"]);
        assert_eq!(read(&tools_dir(&home).join(file("jq"))).as_deref(), Some("jq 1.8.2"));
        assert_eq!(read(&tools_dir(&home).join("LICENSES/jq/COPYING")).as_deref(), Some("jq's"), "its license");
        assert_eq!(read(&tools_dir(&home).join("LICENSES/ripgrep/UNLICENSE")).as_deref(), Some("ripgrep's"), "ripgrep's kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
