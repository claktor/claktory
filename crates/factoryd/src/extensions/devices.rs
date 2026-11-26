//! The Devices extension: an agent adds the person's devices to Claktory for
//! them (`devices_add`), lists them and removes them. Admin agents such as
//! Rivet have it unless it's turned off.
//!
//! Adding a device is Add a device in Settings → Devices, started by the
//! agent: its pairing shows as a card in the agent's conversation after the
//! call (`v1::DevicePairing`), where the person's app shows the code
//! (ShowPairingCode) for them to scan or paste on the device. The code never
//! reaches the agent, its conversation or the logs: its token is kept only in
//! memory (`remote::State::codes`), and only its hash on disk. A device that
//! uses the code waits for the person to allow it, as any other.
//!
//! Pairings live in factory.db. Their state follows devices.json: the card
//! changes as the device uses the code and is allowed, or the code ends, and
//! the agent is told how it ended.

use std::fmt::Write as _;
use std::sync::{Arc, Weak};
use std::time::Duration;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::{self, DevicePairingState as State};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Extension, Manifest, PromptSection, async_tool};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner, RequestError, host_event};
use crate::remote::{self, Device};
use crate::store::Store;
use crate::transcript::{Transcript, now_ms};

pub(crate) const ID: &str = "devices";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Devices",
    icon: "monitor-smartphone",
    description: "Lets it add your devices to Claktory for you, list them and remove them. You still allow each new device yourself.",
    default_enabled: false,
    options: &[],
};

pub(crate) const SCHEMA: &str = "
-- Devices agents add for the person (the Devices extension's devices_add).
CREATE TABLE device_pairings (
    id INTEGER PRIMARY KEY,
    -- The agent that added it.
    agent TEXT NOT NULL,
    -- The device's id in devices.json, and when it was made there: ids are
    -- reused once a device is gone.
    device TEXT NOT NULL,
    created INTEGER NOT NULL,
    name TEXT NOT NULL,
    -- When its code stops working unless a device was allowed with it.
    expires INTEGER NOT NULL,
    -- waiting, asking, added, denied, cancelled or expired
    state TEXT NOT NULL,
    -- When it was added or ended; null while waiting or asking.
    ended INTEGER
);
CREATE INDEX device_pairings_by_agent ON device_pairings (agent, id);
CREATE INDEX device_pairings_by_state ON device_pairings (state);
";

const ADD: &str = "Add a device of the person's (a phone, a laptop) to Claktory, as Settings → Devices → Add a device does. name: what to call it, such as \"Matt's iPhone\". kind: phone, tablet, laptop or desktop, when you know what the device is, so the card leads with the right way to connect (scanning for a phone or tablet, pasting for a computer); leave it out when you don't. The person sees its pairing code in a card in your conversation, for 10 minutes, and scans or pastes it on the device; then they allow the device on this computer themselves. You never see the code. You're told when the device is added, or when the code ends first.";
const LIST: &str = "List the person's devices: those added, with whether they're connected, those waiting for the person to allow them, and codes not used yet.";
const REMOVE: &str = "Remove one of the person's devices, by id or name, only when they ask: it can't connect to Claktory any more, and its open connections close. A device not added yet has its code cancelled.";

/// How often pairings are checked when nothing says devices changed: often
/// enough for a code's end to show within a second.
const CHECK: Duration = Duration::from_secs(1);

pub(crate) struct Devices;

impl Extension for Devices {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        let person = inner.config.person();
        let text = format!(
            "## Devices\n\nYou can add {person}'s devices to Claktory for them, so they can use it there too: devices_add shows {person} a pairing code in your conversation to scan or paste on the device, and {person} allows the device themselves. You never see the code. devices_list lists their devices; devices_remove removes one, only when {person} asks.\n"
        );
        vec![PromptSection::static_text(text)]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let name = json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {"type": "string", "minLength": 1},
                "kind": {"type": "string", "enum": KINDS},
            },
            "additionalProperties": false,
        });
        let none = json!({"type": "object", "properties": {}, "additionalProperties": false});
        let device = json!({
            "type": "object",
            "required": ["device"],
            "properties": {"device": {"type": "string", "minLength": 1}},
            "additionalProperties": false,
        });
        vec![
            async_tool(factory, agent, "devices_add", "Add a device", ADD, name, add),
            async_tool(factory, agent, "devices_list", "List devices", LIST, none, list),
            async_tool(factory, agent, "devices_remove", "Remove a device", REMOVE, device, remove),
        ]
    }

    fn admin_default(&self) -> bool {
        true
    }
}

#[derive(Deserialize)]
struct AddArgs {
    name: String,
    #[serde(default)]
    kind: Option<String>,
}

/// What a device can be said to be (devices_add's kind), as the agent
/// writes it and as `details` carry it.
const KINDS: [&str; 4] = ["phone", "tablet", "laptop", "desktop"];

fn device_kind(word: &str) -> v1::DeviceKind {
    match word {
        "phone" => v1::DeviceKind::Phone,
        "tablet" => v1::DeviceKind::Tablet,
        "laptop" => v1::DeviceKind::Laptop,
        "desktop" => v1::DeviceKind::Desktop,
        _ => v1::DeviceKind::Unspecified,
    }
}

#[derive(Deserialize)]
struct ListArgs {}

#[derive(Deserialize)]
struct RemoveArgs {
    device: String,
}

/// The longest name a device may be given.
const MAX_NAME: usize = 60;

/// Starts adding a device: its code waits for it, shown in the agent's
/// conversation by the card after the call. The result names the pairing,
/// never the code.
async fn add(factory: Arc<Factory>, agent: String, args: AddArgs) -> Result<ToolResult, String> {
    let name = args.name.trim().to_owned();
    if name.is_empty() {
        return Err("Give the device a name, such as \"Laptop\".".to_owned());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("That name is too long: keep it to {MAX_NAME} characters."));
    }
    let kind = match args.kind.as_deref().map(str::trim).filter(|kind| !kind.is_empty()) {
        Some(kind) if KINDS.contains(&kind) => Some(kind.to_owned()),
        Some(kind) => return Err(format!("kind is \"phone\", \"tablet\", \"laptop\" or \"desktop\", or left out; got \"{kind}\".")),
        None => None,
    };
    let person = {
        let inner = factory.lock();
        super::caller(&inner.config, &agent)?;
        inner.config.person_capitalized().to_owned()
    };
    let home = factory.home.clone();
    let paired = tokio::task::spawn_blocking(move || remote::pair(&home, &name)).await.map_err(|error| error.to_string())?;
    let (token, device) = paired.map_err(|error| format!("Couldn't start adding the device: {error}"))?;
    let expires = device.code_expires.unwrap_or_default();
    let id = {
        let db = factory.store.db();
        let added = db.execute(
            "INSERT INTO device_pairings (agent, device, created, name, expires, state) VALUES (?1, ?2, ?3, ?4, ?5, 'waiting')",
            params![agent, device.id, device.created, device.name, expires],
        );
        match added {
            Ok(_) => db.last_insert_rowid().to_string(),
            Err(error) => {
                drop(db);
                let _ = remote::cancel(&factory.home, &device.id);
                return Err(format!("Couldn't record the device: {error}"));
            }
        }
    };
    factory.keep_code(&id, token);
    factory.refresh_remote();
    let minutes = remote::CODE_LIFETIME.as_secs() / 60;
    let said = format!(
        "Started adding {}. {person} sees its code in a card in your conversation for {minutes} minutes, scans or pastes it on the device, then allows the device on {}. You'll be told when it's added, or when the code ends first.",
        device.name,
        remote::machine_name()
    );
    let mut details = json!({"device_pairing": id, "agent": agent, "device": device.id, "name": device.name, "expires_ms": expires});
    if let Some(kind) = kind {
        details["kind"] = kind.into();
    }
    Ok(ToolResult::text(said, details))
}

/// The card a devices_add result shows (called directly, or through
/// extension_call), waiting; None for any other result.
pub(crate) fn started(tool: &str, is_error: bool, details: &Value) -> Option<v1::DevicePairing> {
    if !matches!(tool, "devices_add" | "extension_call") || is_error {
        return None;
    }
    let text = crate::transcript::text_fields(details);
    let id = details.get("device_pairing")?.as_str()?.to_owned();
    Some(v1::DevicePairing {
        id,
        agent_id: text("agent"),
        device_id: text("device"),
        name: text("name"),
        state: State::Waiting.into(),
        expires_ms: details.get("expires_ms").and_then(Value::as_i64).unwrap_or_default(),
        ended_ms: 0,
        kind: device_kind(&text("kind")).into(),
    })
}

async fn list(factory: Arc<Factory>, _agent: String, _args: ListArgs) -> Result<ToolResult, String> {
    let (view, person) = {
        let inner = factory.lock();
        (factory.remote_view(&inner), inner.config.person_capitalized().to_owned())
    };
    let now = now_ms();
    let ago = |since: i64| super::agents::ago(since, now);
    let pending: Vec<&str> = view.pending_devices.iter().map(|device| device.device_id.as_str()).collect();
    let (added, codes): (Vec<&v1::PairedDevice>, Vec<&v1::PairedDevice>) =
        view.devices.iter().filter(|device| !pending.contains(&device.id.as_str())).partition(|device| device.code_expires_ms == 0);
    let mut text = String::new();
    if added.is_empty() {
        text.push_str("No devices added yet.\n");
    } else {
        text.push_str("Added:\n");
        for device in &added {
            let seen = if device.connected {
                "connected now".to_owned()
            } else if device.last_seen_ms > 0 {
                format!("last connected {} ago", ago(device.last_seen_ms))
            } else {
                "never connected".to_owned()
            };
            let _ = writeln!(text, "- {} (id {}): {seen}; added {} ago", device.name, device.id, ago(device.paired_ms));
        }
    }
    if !view.pending_devices.is_empty() {
        let _ = writeln!(text, "Waiting for {person} to allow them:");
        for device in &view.pending_devices {
            let _ = writeln!(text, "- {} (id {}): asked {} ago", device.name, device.device_id, ago(device.requested_ms));
        }
    }
    let codes: Vec<&&v1::PairedDevice> = codes.iter().filter(|device| device.code_expires_ms > now).collect();
    if !codes.is_empty() {
        text.push_str("Codes not used yet:\n");
        for device in codes {
            let left = (device.code_expires_ms - now).max(0) / 60_000 + 1;
            let _ = writeln!(text, "- {} (id {}): ends in {left}m", device.name, device.id);
        }
    }
    Ok(ToolResult::text(text.trim_end().to_owned(), json!({})))
}

async fn remove(factory: Arc<Factory>, _agent: String, args: RemoveArgs) -> Result<ToolResult, String> {
    let wanted = args.device.trim();
    let devices = remote::devices(&factory.home).map_err(|error| format!("Couldn't read the devices: {error}"))?;
    let by_id = devices.iter().find(|device| !device.local && device.id == wanted);
    let named: Vec<&Device> = devices.iter().filter(|device| !device.local && device.name == wanted).collect();
    let device = match (by_id, named.as_slice()) {
        (Some(device), _) => device.clone(),
        (None, [device]) => (*device).clone(),
        (None, []) => return Err(format!("There's no device \"{wanted}\": devices_list shows them.")),
        (None, several) => {
            let ids: Vec<&str> = several.iter().map(|device| device.id.as_str()).collect();
            return Err(format!("Several devices are called \"{wanted}\"; use an id: {}.", ids.join(", ")));
        }
    };
    if device.endpoint_id.is_empty() {
        factory.cancel_pairing(&device.id).await.map_err(|error| error.to_string())?;
        return Ok(ToolResult::text(format!("Cancelled adding {}: its code no longer works.", device.name), json!({})));
    }
    factory.unpair_device(&device.id).await.map_err(|error| error.to_string())?;
    Ok(ToolResult::text(format!("Removed {}: it can't connect to Claktory any more.", device.name), json!({})))
}

/// A pairing as recorded.
struct Row {
    id: i64,
    agent: String,
    device: String,
    created: i64,
    name: String,
    expires: i64,
    state: State,
    ended: Option<i64>,
}

const COLUMNS: &str = "id, agent, device, created, name, expires, state, ended";

impl Row {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
        let state: String = row.get(6)?;
        Ok(Row {
            id: row.get(0)?,
            agent: row.get(1)?,
            device: row.get(2)?,
            created: row.get(3)?,
            name: row.get(4)?,
            expires: row.get(5)?,
            state: parse(&state),
            ended: row.get(7)?,
        })
    }

    fn to_proto(&self) -> v1::DevicePairing {
        v1::DevicePairing {
            id: self.id.to_string(),
            agent_id: self.agent.clone(),
            device_id: self.device.clone(),
            name: self.name.clone(),
            state: self.state.into(),
            expires_ms: self.expires,
            ended_ms: self.ended.unwrap_or_default(),
            // Not recorded here: the shown card keeps what the call said
            // (`Transcript::set_device_pairing` inherits it).
            kind: v1::DeviceKind::Unspecified.into(),
        }
    }

    fn live(&self) -> bool {
        matches!(self.state, State::Waiting | State::Asking)
    }
}

fn parse(state: &str) -> State {
    match state {
        "asking" => State::Asking,
        "added" => State::Added,
        "denied" => State::Denied,
        "cancelled" => State::Cancelled,
        "expired" => State::Expired,
        _ => State::Waiting,
    }
}

fn word(state: State) -> &'static str {
    match state {
        State::Unspecified | State::Waiting => "waiting",
        State::Asking => "asking",
        State::Added => "added",
        State::Denied => "denied",
        State::Cancelled => "cancelled",
        State::Expired => "expired",
    }
}

fn live_rows(store: &Store, agent: Option<&str>) -> rusqlite::Result<Vec<Row>> {
    let db = store.db();
    crate::store::query(
        &db,
        &format!("SELECT {COLUMNS} FROM device_pairings WHERE state IN ('waiting', 'asking') AND (?1 IS NULL OR agent = ?1) ORDER BY id"),
        [agent],
        Row::read,
    )
}

/// Shows the agent's pairings as they are in its just-loaded conversation.
pub(crate) fn restore(store: &Store, agent: &str, transcript: &mut Transcript) -> rusqlite::Result<()> {
    let db = store.db();
    let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM device_pairings WHERE agent = ?1 ORDER BY id"))?;
    for row in statement.query_map([agent], Row::read)? {
        transcript.set_device_pairing(row?.to_proto());
    }
    Ok(())
}

/// Whether a device the agent adds waits for the person.
pub(crate) fn waiting_of(store: &Store, agent: &str) -> bool {
    live_rows(store, Some(agent)).is_ok_and(|rows| !rows.is_empty())
}

/// An agent is deleted: its pairings go, and codes still waiting stop working.
pub(crate) async fn forget(factory: &Factory, agent: &str) -> Result<(), RequestError> {
    let live = live_rows(&factory.store, Some(agent))?;
    for row in &live {
        factory.forget_code(&row.id.to_string());
        let devices = remote::devices(&factory.home).unwrap_or_default();
        if devices.iter().any(|device| device.id == row.device && device.created == row.created && device.endpoint_id.is_empty()) {
            factory.cancel_pairing(&row.device).await?;
        }
    }
    factory.store.db().execute("DELETE FROM device_pairings WHERE agent = ?1", [agent])?;
    Ok(())
}

/// The code of a pairing that waits for its device (ShowPairingCode).
pub(crate) async fn code(factory: &Arc<Factory>, pairing: &str) -> Result<v1::PairingCode, RequestError> {
    let gone = || RequestError::NoPairingCode("This code no longer works: the device was added, or adding it ended.".to_owned());
    let number: i64 = pairing.trim().parse().map_err(|_| gone())?;
    let row =
        factory.store.db().query_row(&format!("SELECT {COLUMNS} FROM device_pairings WHERE id = ?1"), [number], Row::read).optional()?;
    let Some(row) = row.filter(|row| row.state == State::Waiting) else { return Err(gone()) };
    let code = factory.agent_pairing_code(&row.id.to_string(), &row.device, row.created).await?;
    Ok(v1::PairingCode { link: remote::pairing_link(&code), code, device_id: row.device })
}

/// Brings pairings still waiting up to date with devices.json: each card
/// that changed shows it, and the agent hears how an ended one ended.
/// Whether any still waits (or they couldn't be read).
pub(crate) fn sync(factory: &Arc<Factory>) -> bool {
    let live = match live_rows(&factory.store, None) {
        Ok(live) => live,
        Err(error) => {
            tracing::error!(%error, "could not read the devices agents add");
            return true;
        }
    };
    if live.is_empty() {
        return false;
    }
    let devices = match remote::devices(&factory.home) {
        Ok(devices) => devices,
        Err(error) => {
            tracing::error!(%error, "could not read the paired devices");
            return true;
        }
    };
    for mut row in live {
        let state = factory.pairing_state(&devices, &row.device, row.created, row.expires);
        if state == row.state || state == State::Unspecified {
            continue;
        }
        row.state = state;
        row.ended = (!row.live()).then(now_ms);
        let updated = factory
            .store
            .db()
            .execute("UPDATE device_pairings SET state = ?2, ended = ?3 WHERE id = ?1", params![row.id, word(row.state), row.ended]);
        if let Err(error) = updated {
            tracing::error!(%error, pairing = row.id, "could not record how adding a device went");
            continue;
        }
        if state != State::Waiting {
            factory.forget_code(&row.id.to_string());
        }
        {
            let mut inner = factory.lock();
            let entry = inner
                .agents
                .get_mut(&row.agent)
                .and_then(|runtime| runtime.transcript.as_mut())
                .and_then(|transcript| transcript.set_device_pairing(row.to_proto()));
            if let Some(entry) = entry {
                factory.send_change(&inner, &row.agent, crate::transcript::Change::Entry(entry));
                // Whether it waits for the person shows with the agent.
                factory.broadcast_workspace(&inner);
            }
        }
        if !row.live() {
            tell(factory, &row);
        }
    }
    true
}

/// Tells the agent how adding its device ended.
fn tell(factory: &Arc<Factory>, row: &Row) {
    let (person, person_capitalized) = {
        let inner = factory.lock();
        (inner.config.person().to_owned(), inner.config.person_capitalized().to_owned())
    };
    let name = &row.name;
    let summary = match row.state {
        State::Added => format!("{name} was added: {person} allowed it, and it can use Claktory now."),
        State::Denied => format!("{person_capitalized} didn't allow the device that used the code for {name}, so it wasn't added."),
        State::Cancelled => format!("Adding {name} was cancelled, so its code no longer works."),
        State::Expired => format!("The code for {name} ran out before a device was allowed with it, so it wasn't added."),
        State::Unspecified | State::Waiting | State::Asking => return,
    };
    let event = host_event(summary.clone());
    factory.deliver_event(&row.agent, summary, event);
}

/// Keeps agents' pairings up to date: whenever devices may have changed,
/// and every second while one waits (its code runs out); with none waiting,
/// only when they change (#454).
pub(crate) async fn monitor(factory: Weak<Factory>) {
    while let Some(strong) = factory.upgrade() {
        let waiting = sync(&strong);
        let changed = strong.remote.changed.clone();
        drop(strong);
        if waiting {
            let _ = tokio::time::timeout(CHECK, changed.notified()).await;
        } else {
            changed.notified().await;
        }
    }
}
