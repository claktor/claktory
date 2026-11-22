//! The app dock: the installed apps the person pinned, in their order
//! (PinApp, Workspace.pinned_apps), the same on every device (tenet 7).
//!
//! Kept in factory.db's `app_pins`, not config.json, so a backend from
//! before it still reads the settings after a rollback. An app the person
//! unpinned keeps a row without a place, so an update never pins it again;
//! installing an app with views the first time pins it last.

use std::sync::Mutex;

use factory_protocol::v1;
use rusqlite::params;

use super::no_app;
use crate::config::Config;
use crate::factory::{Factory, RequestError};
use crate::store::Store;

pub(crate) const SCHEMA: &str = "
-- The app dock (PinApp): each app the person pinned, at its place (0
-- first), or unpinned (position null), so an update doesn't pin it again.
-- An app uninstalled has no row.
CREATE TABLE app_pins (app TEXT PRIMARY KEY, position INTEGER);
";

/// What `app_pins` holds, read once.
#[derive(Default)]
pub(crate) struct Pins {
    known: Mutex<Option<Known>>,
}

#[derive(Clone, Default)]
struct Known {
    /// Pinned, in order.
    pinned: Vec<String>,
    /// Unpinned by the person.
    unpinned: Vec<String>,
}

impl Pins {
    /// Runs `change` on the pins as factory.db has them, writing them back
    /// when it says they changed.
    fn with<T>(&self, store: &Store, change: impl FnOnce(&mut Known) -> (T, bool)) -> rusqlite::Result<T> {
        let mut known = self.known.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if known.is_none() {
            *known = Some(read(store)?);
        }
        let current = known.as_mut().expect("read above");
        let mut next = current.clone();
        let (out, changed) = change(&mut next);
        if changed {
            write(store, &next)?;
            *current = next;
        }
        Ok(out)
    }
}

fn read(store: &Store) -> rusqlite::Result<Known> {
    let db = store.db();
    let mut statement = db.prepare("SELECT app, position FROM app_pins ORDER BY position, app")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)))?;
    let mut known = Known::default();
    for row in rows {
        let (app, position) = row?;
        if position.is_some() { known.pinned.push(app) } else { known.unpinned.push(app) }
    }
    Ok(known)
}

fn write(store: &Store, known: &Known) -> rusqlite::Result<()> {
    let mut db = store.db();
    let transaction = db.transaction()?;
    transaction.execute("DELETE FROM app_pins", [])?;
    for (position, app) in known.pinned.iter().enumerate() {
        transaction.execute("INSERT INTO app_pins (app, position) VALUES (?1, ?2)", params![app, position as i64])?;
    }
    for app in &known.unpinned {
        transaction.execute("INSERT INTO app_pins (app, position) VALUES (?1, NULL)", params![app])?;
    }
    transaction.commit()
}

/// Workspace.pinned_apps: the pinned apps still installed, in order.
pub(crate) fn view(factory: &Factory, config: &Config) -> Vec<String> {
    if !factory.apps.available() {
        return Vec::new();
    }
    let pinned = factory.apps.pins.with(&factory.store, |known| (known.pinned.clone(), false));
    match pinned {
        Ok(pinned) => pinned.into_iter().filter(|app| config.apps.iter().any(|installed| installed.id == *app)).collect(),
        Err(error) => {
            tracing::warn!(%error, "couldn't read the app dock's pins");
            Vec::new()
        }
    }
}

/// An app was installed: the first time, one with views goes last in the
/// dock.
pub(crate) fn installed(factory: &Factory, app: &str, first: bool, has_views: bool) {
    if !(first && has_views) {
        return;
    }
    let added = factory.apps.pins.with(&factory.store, |known| {
        let new = !known.pinned.iter().chain(&known.unpinned).any(|known| known == app);
        if new {
            known.pinned.push(app.to_owned());
        }
        ((), new)
    });
    if let Err(error) = added {
        tracing::warn!(%error, app, "couldn't pin the app installed");
    }
}

/// An app was uninstalled: it leaves the dock, and a later install is a
/// first one again.
pub(crate) fn uninstalled(factory: &Factory, app: &str) {
    let removed = factory.apps.pins.with(&factory.store, |known| {
        let before = known.pinned.len() + known.unpinned.len();
        known.pinned.retain(|pinned| pinned != app);
        known.unpinned.retain(|unpinned| unpinned != app);
        ((), known.pinned.len() + known.unpinned.len() != before)
    });
    if let Err(error) = removed {
        tracing::warn!(%error, app, "couldn't unpin the app uninstalled");
    }
}

impl Factory {
    /// PinApp: pins an installed app at a place in the dock, moves it, or
    /// unpins it.
    pub(crate) fn pin_app(&self, request: &v1::PinApp) -> Result<(), RequestError> {
        if !self.lock().config.apps.iter().any(|app| app.id == request.app_id) {
            return Err(no_app(&request.app_id));
        }
        let app = request.app_id.clone();
        self.apps.pins.with(&self.store, |known| {
            let before = (known.pinned.clone(), known.unpinned.clone());
            known.pinned.retain(|pinned| *pinned != app);
            known.unpinned.retain(|unpinned| *unpinned != app);
            if request.pinned {
                let at = usize::try_from(request.position).map_or(known.pinned.len(), |at| at.min(known.pinned.len()));
                known.pinned.insert(at, app.clone());
            } else {
                known.unpinned.push(app.clone());
            }
            ((), (known.pinned.clone(), known.unpinned.clone()) != before)
        })?;
        self.apps_changed();
        Ok(())
    }
}
