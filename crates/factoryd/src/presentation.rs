//! Where each device was reading (docs/ipad-phase3-contract.md §1): per
//! device and agent, the pane in focus and each pane's place (the page a PDF
//! was on), and per device whether it hides the pinned sidebar. Each device
//! keeps its own, as each keeps its own zoom: reading a paper on the iPad
//! never moves the Mac.
//!
//! A device's record goes only to that device's connections (`Port`),
//! never to another device or an agent. Panes a layout no longer has are
//! dropped from every record, and a deleted agent's or unpaired device's
//! records go with it.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use factory_protocol::v1::{self, server_message::Body, view_node::Node};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Where each device was reading an agent's panes (crate::presentation).
CREATE TABLE presentations (
    -- The device's id in remote/devices.json; 'local' for this machine's app.
    device TEXT NOT NULL,
    agent TEXT NOT NULL,
    -- The pane in focus: a ViewNode id in the agent's layout, or ''.
    focused_leaf TEXT NOT NULL,
    -- Each pane's place, as JSON: [{\"leaf\": \"pane-2\", \"page\": 3}].
    places TEXT NOT NULL,
    updated INTEGER NOT NULL,
    PRIMARY KEY (device, agent)
);
-- Whether each device hides the pinned sidebar.
CREATE TABLE presentation_sidebars (
    device TEXT PRIMARY KEY,
    hidden INTEGER NOT NULL,
    updated INTEGER NOT NULL
);
";

/// The most places a record keeps, and the longest pane id.
const PLACES_MOST: usize = 64;
const LEAF_ID_MOST: usize = 128;
/// The highest page kept.
const PAGE_MOST: u32 = 100_000;

fn refused(why: impl Into<String>) -> RequestError {
    RequestError::Ink(why.into())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Place {
    leaf: String,
    page: u32,
}

/// The ids of an agent's panes now; empty when it shows only its
/// conversation.
fn leaves(factory: &Factory, agent: &str) -> BTreeSet<String> {
    fn walk(node: &v1::ViewNode, into: &mut BTreeSet<String>) {
        match &node.node {
            Some(Node::Leaf(_)) => {
                into.insert(node.id.clone());
            }
            Some(Node::Split(split)) => split.children.iter().for_each(|child| walk(child, into)),
            None => {}
        }
    }
    let mut ids = BTreeSet::new();
    if let Some(root) = factory.viewer.layout(agent) {
        walk(&root, &mut ids);
    }
    ids
}

/// The connections of each device, waiting for its record.
#[derive(Default)]
pub(crate) struct Hub {
    devices: Mutex<HashMap<String, watch::Sender<v1::Presentation>>>,
}

impl Hub {
    fn sender(&self, device: &str) -> watch::Sender<v1::Presentation> {
        let mut devices = self.devices.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        devices.entry(device.to_owned()).or_insert_with(|| watch::channel(v1::Presentation::default()).0).clone()
    }
}

/// One connection's view of its device's record.
pub(crate) struct Port {
    receiver: tokio::sync::Mutex<watch::Receiver<v1::Presentation>>,
}

impl Port {
    /// Made before the connection's first Presentation is read, so a change
    /// meanwhile still comes.
    pub(crate) fn new(factory: &Factory, device: &str) -> Port {
        let mut receiver = factory.presentations.sender(device).subscribe();
        receiver.mark_unchanged();
        Port { receiver: tokio::sync::Mutex::new(receiver) }
    }

    /// The device's record whenever it changes.
    pub(crate) async fn next(&self) -> Body {
        let mut receiver = self.receiver.lock().await;
        match receiver.changed().await {
            Ok(()) => Body::Presentation(receiver.borrow_and_update().clone()),
            Err(_) => std::future::pending().await,
        }
    }
}

/// Device `device`'s record, every agent's.
pub(crate) fn of(db: &Connection, device: &str) -> rusqlite::Result<v1::Presentation> {
    let mut statement = db.prepare("SELECT agent, focused_leaf, places FROM presentations WHERE device = ?1 ORDER BY agent")?;
    let agents = statement
        .query_map([device], |row| {
            let places: String = row.get(2)?;
            let places: Vec<Place> = serde_json::from_str(&places).unwrap_or_default();
            Ok(v1::AgentPresentation {
                agent_id: row.get(0)?,
                focused_leaf_id: row.get(1)?,
                places: places.into_iter().map(|place| v1::LeafPlace { leaf_id: place.leaf, page_1based: place.page }).collect(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    let sidebar_hidden =
        db.query_row("SELECT hidden FROM presentation_sidebars WHERE device = ?1", [device], |row| row.get(0)).optional()?.unwrap_or(false);
    Ok(v1::Presentation { agents, sidebar_hidden })
}

/// Sends `devices`' records to their connections.
fn tell(factory: &Factory, db: &Connection, devices: impl IntoIterator<Item = String>) {
    for device in devices {
        match of(db, &device) {
            Ok(presentation) => {
                factory.presentations.sender(&device).send_replace(presentation);
            }
            Err(error) => tracing::warn!(%error, device, "couldn't read a device's reading place"),
        }
    }
}

/// Writes one record, or removes it when it holds nothing.
fn put(db: &Connection, device: &str, agent: &str, focused: &str, places: &[Place]) -> rusqlite::Result<()> {
    if focused.is_empty() && places.is_empty() {
        db.execute("DELETE FROM presentations WHERE device = ?1 AND agent = ?2", params![device, agent])?;
    } else {
        db.execute(
            "INSERT INTO presentations (device, agent, focused_leaf, places, updated) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (device, agent) DO UPDATE SET focused_leaf = excluded.focused_leaf, places = excluded.places, updated = excluded.updated",
            params![device, agent, focused, serde_json::to_string(places).unwrap_or_else(|_| "[]".to_owned()), now_ms()],
        )?;
    }
    Ok(())
}

/// SetPresentation, from `device`'s connection.
pub(crate) fn set(factory: &Factory, device: &str, request: &v1::SetPresentation) -> Result<(), RequestError> {
    let agent = request.agent_id.trim();
    if agent.is_empty() && request.sidebar_hidden.is_none() {
        return Err(RequestError::Empty("agent_id"));
    }
    let mut places = Vec::new();
    if !agent.is_empty() {
        factory.known_agent(agent)?;
        if request.focused_leaf_id.len() > LEAF_ID_MOST || request.places.len() > PLACES_MOST {
            return Err(refused(format!("A reading place has at most {PLACES_MOST} panes, each id up to {LEAF_ID_MOST} characters.")));
        }
        let mut seen = BTreeSet::new();
        for place in &request.places {
            if place.leaf_id.is_empty() || place.leaf_id.len() > LEAF_ID_MOST || !seen.insert(place.leaf_id.as_str()) {
                return Err(refused("Each place names a different pane."));
            }
            if place.page_1based > PAGE_MOST {
                return Err(refused(format!("A page is 1 to {PAGE_MOST}.")));
            }
            // Nothing to keep yet but the page.
            if place.page_1based > 0 {
                places.push(Place { leaf: place.leaf_id.clone(), page: place.page_1based });
            }
        }
    }
    {
        let db = factory.store.db();
        if !agent.is_empty() {
            // Only panes the layout has; a layout changing meanwhile is
            // caught by `prune` below.
            let leaves = leaves(factory, agent);
            let focused = if leaves.contains(&request.focused_leaf_id) { request.focused_leaf_id.as_str() } else { "" };
            places.retain(|place| leaves.contains(&place.leaf));
            put(&db, device, agent, focused, &places)?;
        }
        if let Some(hidden) = request.sidebar_hidden {
            db.execute(
                "INSERT INTO presentation_sidebars (device, hidden, updated) VALUES (?1, ?2, ?3)
                 ON CONFLICT (device) DO UPDATE SET hidden = excluded.hidden, updated = excluded.updated",
                params![device, hidden, now_ms()],
            )?;
        }
        tell(factory, &db, [device.to_owned()]);
    }
    if !agent.is_empty() {
        layout_changed(factory, agent);
    }
    Ok(())
}

/// The agent's layout changed: panes it no longer has leave every device's
/// record, and those devices hear of it.
pub(crate) fn layout_changed(factory: &Factory, agent: &str) {
    let leaves = leaves(factory, agent);
    let db = factory.store.db();
    let changed = (|| -> rusqlite::Result<Vec<String>> {
        let records: Vec<(String, String, String)> = {
            let mut statement = db.prepare("SELECT device, focused_leaf, places FROM presentations WHERE agent = ?1")?;
            statement.query_map([agent], crate::store::row3)?.collect::<rusqlite::Result<_>>()?
        };
        let mut changed = Vec::new();
        for (device, focused, places) in records {
            let places: Vec<Place> = serde_json::from_str(&places).unwrap_or_default();
            let kept: Vec<Place> = places.iter().filter(|place| leaves.contains(&place.leaf)).cloned().collect();
            let focus = if leaves.contains(&focused) { focused.as_str() } else { "" };
            if focus != focused || kept != places {
                put(&db, &device, agent, focus, &kept)?;
                changed.push(device);
            }
        }
        Ok(changed)
    })();
    match changed {
        Ok(changed) => tell(factory, &db, changed),
        Err(error) => tracing::warn!(%error, agent, "couldn't drop panes a layout no longer has from reading places"),
    }
}

/// An agent is deleted: every device's record of it goes.
pub(crate) fn forget_agent(factory: &Factory, agent: &str) -> rusqlite::Result<()> {
    let db = factory.store.db();
    let devices: Vec<String> = {
        let mut statement = db.prepare("SELECT device FROM presentations WHERE agent = ?1")?;
        statement.query_map([agent], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?
    };
    db.execute("DELETE FROM presentations WHERE agent = ?1", [agent])?;
    tell(factory, &db, devices);
    Ok(())
}

/// A device is unpaired: its records go.
pub(crate) fn forget_device(factory: &Factory, device: &str) -> rusqlite::Result<()> {
    let db = factory.store.db();
    db.execute("DELETE FROM presentations WHERE device = ?1", [device])?;
    db.execute("DELETE FROM presentation_sidebars WHERE device = ?1", [device])?;
    factory.presentations.devices.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(device);
    Ok(())
}

/// Device `device`'s record for a connection that just came; none when it
/// keeps nothing (the app starts from nothing anyway).
pub(crate) fn welcome(factory: &Factory, device: &str) -> Option<v1::Presentation> {
    match of(&factory.store.db(), device) {
        Ok(presentation) => (presentation != v1::Presentation::default()).then_some(presentation),
        Err(error) => {
            tracing::warn!(%error, device, "couldn't read a device's reading place");
            None
        }
    }
}
