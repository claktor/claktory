//! Needs you's focus mode, per device (#514, spec §9.1): which devices
//! are in it, so each app opens as it left it. Kept in factory.db (it
//! changes with every ⌘⇧N, so not config.json) and shown in
//! Settings.focus_mode; each device sets only its own (SetFocusMode, the
//! connection's own device, never one a request names), and a removed
//! device leaves it.

use std::collections::BTreeSet;
use std::sync::Mutex;

use crate::factory::{Factory, RequestError};

pub(crate) const SCHEMA: &str = "
-- The devices in Needs you's focus mode (focus_mode.rs), by id as
-- Welcome.device_id gives it ('local' for this computer's app).
CREATE TABLE focus_mode (device TEXT PRIMARY KEY);
";

/// What factory.db has, read once.
#[derive(Default)]
pub(crate) struct FocusMode {
    devices: Mutex<Option<BTreeSet<String>>>,
}

impl FocusMode {
    fn with<R>(&self, factory: &Factory, act: impl FnOnce(&mut BTreeSet<String>) -> R) -> R {
        let mut devices = self.devices.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let devices = devices.get_or_insert_with(|| read(factory));
        act(devices)
    }

    /// Settings.focus_mode.
    pub(crate) fn devices(&self, factory: &Factory) -> Vec<String> {
        self.with(factory, |devices| devices.iter().cloned().collect())
    }
}

fn read(factory: &Factory) -> BTreeSet<String> {
    let db = factory.store.db();
    let devices = db
        .prepare("SELECT device FROM focus_mode ORDER BY device")
        .and_then(|mut statement| statement.query_map([], |row| row.get(0))?.collect::<rusqlite::Result<BTreeSet<String>>>());
    devices.unwrap_or_else(|error| {
        tracing::error!(%error, "could not read which devices are in focus mode");
        BTreeSet::new()
    })
}

/// SetFocusMode: `device` (the connection's own) in focus mode, or out.
pub(crate) fn set(factory: &Factory, device: &str, on: bool) -> Result<(), RequestError> {
    let changed = factory.focus_mode.with(factory, |devices| {
        let changed = if on { devices.insert(device.to_owned()) } else { devices.remove(device) };
        if changed {
            let db = factory.store.db();
            if on {
                db.execute("INSERT OR IGNORE INTO focus_mode (device) VALUES (?1)", [device])?;
            } else {
                db.execute("DELETE FROM focus_mode WHERE device = ?1", [device])?;
            }
        }
        Ok::<_, rusqlite::Error>(changed)
    })?;
    if changed {
        factory.broadcast_workspace(&factory.lock());
    }
    Ok(())
}

/// Devices removed: they leave focus mode.
pub(crate) fn forget(factory: &Factory, removed: &[String]) {
    let changed = factory.focus_mode.with(factory, |devices| {
        let mut changed = false;
        for device in removed {
            if devices.remove(device) {
                changed = true;
                if let Err(error) = factory.store.db().execute("DELETE FROM focus_mode WHERE device = ?1", [device]) {
                    tracing::warn!(%error, %device, "could not take a removed device out of focus mode");
                }
            }
        }
        changed
    });
    if changed {
        factory.broadcast_workspace(&factory.lock());
    }
}
