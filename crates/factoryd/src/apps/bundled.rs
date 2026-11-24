//! Claktory's own apps, shipped with each release (#201): signed .clapp
//! files in an `apps` folder beside the release's programs. Claktory
//! installs each by itself (#333), granting it nothing (no data, no agent),
//! unless the person uninstalled it (`app_uninstalled`, in factory.db so a
//! rollback's config.json is untouched), and updates it when a newer one
//! ships that asks for nothing new but data (which grants nothing either);
//! otherwise the Marketplace's Apps tab offers Install, or Update when the
//! installed version is older. Being in the folder vouches for nothing: each file is read and
//! checked like any other (`Bundle::read`: its signature and manifest), and
//! offered only when its key is Claktory's own (`CLAKTORY_KEYS`); one that
//! fails isn't listed, and installing one goes through the same review and
//! install as "Install from a file…" (`install_local_app`), its file read
//! and checked again then.
//!
//! The folder, the first that exists: `FACTORY_BUNDLED_APPS`; the running
//! version's release (`<home>/releases/<version>/apps`, which an update and
//! a roll-back always match); beside factoryd (`bin/apps`, put there by the
//! installers, or the Mac app's `Contents/Helpers/apps`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use factory_protocol::v1;

use super::bundle::{self, Bundle, Manifest, Tier};
use crate::config::Config;
use crate::factory::Factory;
use crate::store::Store;

pub(crate) const SCHEMA: &str = "
-- Claktory's own apps (shipped with each release) the person uninstalled:
-- Claktory doesn't install them again by itself. Installing one clears it.
CREATE TABLE app_uninstalled (app TEXT PRIMARY KEY, at INTEGER NOT NULL);
";

/// The folder beside the programs, and the name each app's file has in it.
const FOLDER: &str = "apps";
const EXTENSION: &str = "clapp";

/// A bundled app that passed its checks.
#[derive(Clone)]
pub(crate) struct Offer {
    pub path: PathBuf,
    pub manifest: Arc<Manifest>,
    /// Its publisher's key, base64: always one of Claktory's.
    pub key: String,
}

/// The offers, read once (a release's folder doesn't change while it runs).
#[derive(Default)]
pub(crate) struct Bundled {
    read: Mutex<Option<Vec<Offer>>>,
    /// The offers read were installed or updated where they should be
    /// (`keep_installed`): once each time they're read.
    kept: std::sync::atomic::AtomicBool,
    /// Tests: the folder to read instead.
    folder: Mutex<Option<PathBuf>>,
}

impl Bundled {
    /// What the folder offers, read and checked the first time.
    pub(crate) fn offers(&self, factory: &Factory) -> Vec<Offer> {
        let mut read = self.read.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        read.get_or_insert_with(|| scan(self.folder(factory))).clone()
    }

    fn folder(&self, factory: &Factory) -> Option<PathBuf> {
        if let Some(folder) = self.folder.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() {
            return Some(folder);
        }
        if let Some(folder) = std::env::var_os("FACTORY_BUNDLED_APPS").filter(|folder| !folder.is_empty()) {
            return Some(PathBuf::from(folder));
        }
        let release = factory.home.releases().join(crate::update::VERSION).join(FOLDER);
        let beside = crate::update::running_exe().and_then(|exe| Some(exe.parent()?.join(FOLDER)));
        [Some(release), beside].into_iter().flatten().find(|folder| folder.is_dir())
    }
}

/// Every .clapp in `folder` that reads, checks and is signed with one of
/// Claktory's keys, by name; one per app id (the newest).
fn scan(folder: Option<PathBuf>) -> Vec<Offer> {
    let Some(folder) = folder else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(&folder) else { return Vec::new() };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == EXTENSION) && path.is_file())
        .collect();
    paths.sort();
    let mut offers: Vec<Offer> = Vec::new();
    for path in paths {
        let bundle = match read(&path) {
            Ok(bundle) => bundle,
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "a bundled app didn't pass its checks; it isn't offered");
                continue;
            }
        };
        let offer = Offer { path, manifest: Arc::new(bundle.manifest), key: bundle.key };
        match offers.iter_mut().find(|kept| kept.manifest.id == offer.manifest.id) {
            Some(kept) if newer(&offer.manifest.version, &kept.manifest.version) => *kept = offer,
            Some(_) => {}
            None => offers.push(offer),
        }
    }
    offers.sort_by_key(|offer| offer.manifest.name.to_lowercase());
    offers
}

/// The file at `path`, read and checked as any app is, and Claktory's own.
pub(crate) fn read(path: &std::path::Path) -> Result<Bundle, String> {
    read_bytes(path).map(|(bundle, _)| bundle)
}

/// The file at `path` and its bytes, checked: the bytes are what an
/// install goes on with, so what was checked is what's installed.
pub(crate) fn read_bytes(path: &std::path::Path) -> Result<(Bundle, Vec<u8>), String> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > bundle::MAX_BYTES as u64 => {
            return Err(format!("This file is too big for an app (over {} MB).", bundle::MAX_BYTES / 1024 / 1024));
        }
        Ok(_) => {}
        Err(error) => return Err(format!("Claktory's copy of this app can't be read: {error}")),
    }
    let bytes = std::fs::read(path).map_err(|error| format!("Claktory's copy of this app can't be read: {error}"))?;
    let bundle = Bundle::read(&bytes)?;
    if bundle.tier != Tier::Claktory {
        return Err("This app isn't signed by Claktory, so it isn't offered as one of Claktory's own.".to_owned());
    }
    Ok((bundle, bytes))
}

/// Whether the person uninstalled the bundled app `id`.
fn uninstalled(store: &Store, id: &str) -> bool {
    store.db().query_row("SELECT 1 FROM app_uninstalled WHERE app = ?1", [id], |_| Ok(())).is_ok()
}

/// Notes that the person uninstalled `id`, one of Claktory's own apps, or
/// (`gone` false) that it's installed again.
pub(crate) fn note_uninstalled(factory: &Factory, id: &str, key: &str, gone: bool) {
    let db = factory.store.db();
    let done = if !gone {
        db.execute("DELETE FROM app_uninstalled WHERE app = ?1", [id])
    } else if factory.apps.bundled.offers(factory).iter().any(|offer| offer.manifest.id == id && offer.key == key) {
        db.execute("INSERT OR REPLACE INTO app_uninstalled (app, at) VALUES (?1, ?2)", rusqlite::params![id, crate::transcript::now_ms()])
    } else {
        return;
    };
    if let Err(error) = done {
        tracing::error!(app = %id, %error, "could not note whether the person uninstalled one of Claktory's apps");
    }
}

/// Whether `after` asks for anything `before` didn't but data, which
/// installing or updating grants nothing of: then the person reviews it.
fn asks_more(before: &Manifest, after: &Manifest) -> bool {
    after.hosts.iter().any(|host| !before.hosts.contains(host))
        || after
            .scopes
            .iter()
            .any(|scope| before.scope(&scope.id).is_none_or(|old| scope.tools.iter().any(|tool| !old.tools.contains(tool))))
        || after.events.iter().any(|event| before.event(&event.kind).is_none())
        || after.secrets.iter().any(|secret| before.secret(&secret.name).is_none())
        || after.memory_mb > before.memory_mb
        || after.storage_mb > before.storage_mb
}

/// Installs each bundled app that isn't installed and that the person
/// didn't uninstall, and updates each that's older when the new one asks
/// for nothing new but data, through the same checks as any install
/// (`install_local_app`, its file read and checked again). Nothing is
/// granted: a new install reads no data and no agent has it. Once each time
/// the offers are read.
pub(crate) async fn keep_installed(factory: &Arc<Factory>) {
    if !factory.apps.available() || factory.apps.bundled.kept.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let offers = factory.apps.bundled.offers(factory);
    for offer in offers {
        let id = offer.manifest.id.clone();
        let installed = factory.lock().config.apps.iter().find(|app| app.id == id).cloned();
        match &installed {
            None if uninstalled(&factory.store, &id) => continue,
            None => {}
            Some(app) if app.key != offer.key || !newer(&offer.manifest.version, &app.version) => continue,
            Some(_) => {
                let more = factory.apps.manifest(&id).is_none_or(|before| asks_more(&before, &offer.manifest));
                if more {
                    tracing::info!(app = %id, version = %offer.manifest.version, "a newer bundled app asks for more: offered for the person to review");
                    continue;
                }
            }
        }
        let ask = v1::InstallLocalApp { bundled: id.clone(), ..Default::default() };
        let reviewed = match factory.install_local_app(&ask).await {
            Ok(Some(reviewed)) => reviewed,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(app = %id, %error, "couldn't read a bundled app to install it");
                continue;
            }
        };
        let approve = v1::InstallLocalApp { bundled: id.clone(), approve: true, sha256: reviewed.sha256, ..Default::default() };
        match factory.install_local_app(&approve).await {
            Ok(_) if installed.is_none() => {
                tracing::info!(app = %id, version = %offer.manifest.version, "installed one of Claktory's own apps, granting it nothing")
            }
            Ok(_) => tracing::info!(app = %id, version = %offer.manifest.version, "updated one of Claktory's own apps"),
            Err(error) => tracing::warn!(app = %id, %error, "couldn't install a bundled app"),
        }
    }
}

/// Whether the installed `app` is one of the bundled ones (AppInfo.bundled).
pub(crate) fn is_bundled(factory: &Factory, id: &str, key: &str) -> bool {
    factory.apps.available() && factory.apps.bundled.offers(factory).iter().any(|offer| offer.manifest.id == id && offer.key == key)
}

/// Whether app version `candidate` is newer than `than` ("0.2.1" > "0.2.0").
fn newer(candidate: &str, than: &str) -> bool {
    crate::update::newer(&format!("v{candidate}"), &format!("v{than}"))
}

/// The file of the bundled app `id`, to install.
pub(crate) fn path(factory: &Factory, id: &str) -> Option<PathBuf> {
    factory.apps.bundled.offers(factory).into_iter().find(|offer| offer.manifest.id == id).map(|offer| offer.path)
}

/// Whether `offer` is offered next to what's installed with its id
/// (version, key): Some("") when nothing is, Some(the installed version)
/// for an update, None when it's as new or older, or another publisher's
/// app has the id (theirs stays).
fn offered(offer: &Offer, installed: Option<(&str, &str)>) -> Option<String> {
    match installed {
        None => Some(String::new()),
        Some((_, key)) if key != offer.key => None,
        Some((version, _)) if newer(&offer.manifest.version, version) => Some(version.to_owned()),
        Some(_) => None,
    }
}

/// Workspace.bundled_apps: those not installed (saying whether the person
/// uninstalled them), or newer than the version installed from the same
/// publisher; none while apps aren't offered.
pub(crate) fn view(factory: &Factory, config: &Config) -> Vec<v1::BundledApp> {
    if !factory.apps.available() {
        return Vec::new();
    }
    factory
        .apps
        .bundled
        .offers(factory)
        .into_iter()
        .filter_map(|offer| {
            let installed = config.apps.iter().find(|app| app.id == offer.manifest.id);
            let installed_version = offered(&offer, installed.map(|app| (app.version.as_str(), app.key.as_str())))?;
            let manifest = &offer.manifest;
            Some(v1::BundledApp {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                description: manifest.description.clone(),
                icon: manifest.icon.clone(),
                version: manifest.version.clone(),
                publisher: manifest.publisher.clone(),
                uninstalled: installed.is_none() && uninstalled(&factory.store, &manifest.id),
                installed_version,
            })
        })
        .collect()
}

/// Tests: bundled apps come from `folder` (read again).
#[doc(hidden)]
pub fn set_folder_for_test(factory: &Factory, folder: PathBuf) {
    *factory.apps.bundled.folder.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(folder);
    *factory.apps.bundled.read.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    factory.apps.bundled.kept.store(false, std::sync::atomic::Ordering::Release);
    factory.apps.poke();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_app_is_offered_when_missing_or_older_and_never_over_another_publishers() {
        let manifest =
            Manifest::parse("id = \"charts\"\nname = \"Charts\"\nversion = \"0.2.1\"\nworld = \"claktory:app@1\"\n").expect("manifest");
        let offer = Offer { path: PathBuf::from("charts.clapp"), manifest: Arc::new(manifest), key: "claktory".into() };
        assert_eq!(offered(&offer, None).as_deref(), Some(""));
        assert_eq!(offered(&offer, Some(("0.2.0", "claktory"))).as_deref(), Some("0.2.0"));
        assert_eq!(offered(&offer, Some(("0.2.1", "claktory"))), None);
        assert_eq!(offered(&offer, Some(("0.3.0", "claktory"))), None);
        assert_eq!(offered(&offer, Some(("0.1.0", "someone else"))), None);
    }

    #[test]
    fn app_versions_compare_by_number() {
        assert!(newer("0.2.1", "0.2.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("0.2.0", "0.2.0"));
        assert!(!newer("0.1.9", "0.2.0"));
    }
}
