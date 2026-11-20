//! Devices reach Factory through its private tunnel (iroh): an endpoint with
//! a key of its own (`<home>/remote/iroh.key`), dialled by that key, end to
//! end encrypted, directly or through a relay that can't read it. Factory
//! listens on no port for them. Each app connection is one bidirectional
//! stream on ALPN `claktory/1`, carrying a WebSocket and then the protocol,
//! as on the loopback port.
//!
//! - Each paired device has a token, given once in its pairing code; Factory
//!   keeps only the token's SHA-256 (`<home>/remote/devices.json`), read
//!   afresh at every handshake, so `factoryd unpair` applies at once.
//! - The first device to connect with a code binds its key to it; from then
//!   on only that key may use it. A code nobody used expires.
//! - Right after iroh's handshake, a key that isn't a paired device's is
//!   turned away, before any of Factory's protocol, unless a code is waiting
//!   for its device or it connects from this machine.
//! - This machine's app connects the same way, with the code Factory writes
//!   to `<home>/remote/local-pairing` (only this user can read it), so the
//!   endpoint always runs: on 127.0.0.1 only while no other device can
//!   connect, on every address and with relays while one can (remote access
//!   is on).
//! - A device on another machine can't make Factory replace an app bundle on
//!   this machine (`InstallUpdate.app_path` is ignored for it).
//!
//! The loopback port (7420) wants the local pairing code's token too. Without
//! one it lets a client in only until this machine's app has connected
//! through the tunnel once: an app from before the tunnel keeps working
//! through an update, and after that an agent on this machine can't drive
//! Factory without the token. (It can read the token's file, as it runs as
//! the same user: a bar, not a boundary.)

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use factory_protocol::v1;
use iroh::endpoint::{AfterHandshakeOutcome, BindOpts, Builder, Connection, EndpointHooks, presets};
use iroh::{Endpoint, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{Config, RemoteConfig};
use crate::factory::{Factory, Inner, RequestError};
use crate::home::Home;
use crate::transcript::now_ms;

/// The backend's protocol on iroh, the same in the app (app/rust).
pub const ALPN: &[u8] = b"claktory/1";
/// How often the tunnel checks whether it should change (a device paired or
/// unpaired, a code expired, the relays changed) and what it shows.
const CHECK: Duration = Duration::from_secs(1);
/// A device that gave a wrong token waits at least this long for the answer
/// to its next try.
const FAILURE_PAUSE: Duration = Duration::from_secs(1);
/// A pairing code nobody used stops working after this long.
pub const CODE_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// How long making a pairing code waits for the tunnel to reach its relay,
/// so the code can name it.
const ONLINE_WAIT: Duration = Duration::from_secs(5);
/// This machine's own app, in devices.json.
pub const LOCAL_DEVICE: &str = "local";
/// What a turned-away connection is closed with.
const REFUSED: u32 = 1;
/// What an unpaired device's connection is closed with.
const UNPAIRED: u32 = 2;

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0} is broken: {1}")]
    Devices(PathBuf, serde_json::Error),
    #[error("couldn't make a token: {0}")]
    Random(#[from] getrandom::Error),
    #[error("{0} isn't valid: {1}")]
    Config(PathBuf, String),
    #[error("no device named \"{0}\" is paired")]
    NoDevice(String),
    #[error("{0} isn't a key Claktory can read: delete it, and Claktory makes a new one (devices then need pairing again)")]
    BadKey(PathBuf),
    #[error("the private tunnel can't start: {0}")]
    Tunnel(String),
}

/// Where Factory keeps what devices need.
fn dir(home: &Home) -> PathBuf {
    home.root().join("remote")
}

/// Written so only this user can read it.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    // Its own name per write, so two writers never rename each other's.
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let partial = path.with_extension(format!("partial-{}-{}", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(partial, path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len()).step_by(2).map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok()).collect()
}

/// Factory's iroh key, made the first time: its public half is the
/// endpoint id devices dial.
pub fn secret_key(home: &Home) -> Result<SecretKey, RemoteError> {
    let path = dir(home).join("iroh.key");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let bytes: [u8; 32] = unhex(&text).and_then(|bytes| bytes.try_into().ok()).ok_or(RemoteError::BadKey(path))?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let key = SecretKey::generate();
            std::fs::create_dir_all(dir(home))?;
            write_private(&path, format!("{}\n", hex(&key.to_bytes())).as_bytes())?;
            Ok(key)
        }
        Err(error) => Err(error.into()),
    }
}

/// A device paired with this Factory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// The SHA-256 of its token, in hex; the token itself isn't kept.
    pub token_sha256: String,
    /// When it was paired, in ms since 1970.
    pub created: i64,
    /// When it last connected; None if it never has.
    #[serde(default)]
    pub last_seen: Option<i64>,
    /// The IP address it last connected from; empty when a relay carried it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_address: String,
    /// The key it connects with (its iroh endpoint id), bound when it first
    /// connected; empty until then.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint_id: String,
    /// Until its code is used: when the code stops working, in ms since 1970.
    /// A device paired before the tunnel has neither this nor a key, and
    /// can't connect until it's paired again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_expires: Option<i64>,
    /// This machine's own app (`LOCAL_DEVICE`), which Devices doesn't list.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
}

impl Device {
    /// Its code is waiting for its device: not used yet and not expired.
    fn awaiting(&self, now: i64) -> bool {
        !self.local && self.endpoint_id.is_empty() && self.code_expires.is_some_and(|expires| expires > now)
    }

    /// It can connect from another machine.
    fn usable(&self, now: i64) -> bool {
        !self.local && (!self.endpoint_id.is_empty() || self.awaiting(now))
    }
}

/// Remote access is on while a device on another machine can connect.
fn remote_on(devices: &[Device], now: i64) -> bool {
    devices.iter().any(|device| device.usable(now))
}

fn devices_path(home: &Home) -> PathBuf {
    dir(home).join("devices.json")
}

pub fn devices(home: &Home) -> Result<Vec<Device>, RemoteError> {
    let path = devices_path(home);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| RemoteError::Devices(path, error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

/// Held while devices.json is read, changed and written, so changes made at
/// once (a pairing while the tunnel starts) don't undo each other.
fn devices_lock() -> std::sync::MutexGuard<'static, ()> {
    static DEVICES: Mutex<()> = Mutex::new(());
    lock(&DEVICES)
}

fn save_devices(home: &Home, devices: &[Device]) -> Result<(), RemoteError> {
    std::fs::create_dir_all(dir(home))?;
    let mut json = serde_json::to_string_pretty(devices).map_err(|error| RemoteError::Devices(devices_path(home), error))?;
    json.push('\n');
    write_private(&devices_path(home), json.as_bytes())?;
    Ok(())
}

/// config.json's `remote` as it is on disk; None when it has none.
fn remote_config(home: &Home) -> Result<Option<RemoteConfig>, RemoteError> {
    let path = home.config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let config = Config::parse(&text).map_err(|error| RemoteError::Config(path.clone(), error.to_string()))?;
    Ok(config.remote)
}

fn new_token() -> Result<String, RemoteError> {
    let mut token = [0_u8; 32];
    getrandom::fill(&mut token)?;
    Ok(hex(&token))
}

fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

/// Pairs a new device: makes its token, good for `CODE_LIFETIME` until the
/// device first connects. Returns the token, which its pairing code holds,
/// so it's shown once and never again, and the device.
pub fn pair(home: &Home, name: &str) -> Result<(String, Device), RemoteError> {
    let token = new_token()?;
    let _changing = devices_lock();
    let mut devices = devices(home)?;
    let name = name.trim();
    let base: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let base = base.trim_matches('-');
    let base = if base.is_empty() || base == LOCAL_DEVICE { "device" } else { base };
    let mut id = base.to_owned();
    let mut n = 2;
    while devices.iter().any(|device| device.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    let now = now_ms();
    let device = Device {
        id,
        name: if name.is_empty() { base.to_owned() } else { name.to_owned() },
        token_sha256: token_hash(&token),
        created: now,
        last_seen: None,
        last_address: String::new(),
        endpoint_id: String::new(),
        code_expires: Some(now + CODE_LIFETIME.as_millis() as i64),
        local: false,
    };
    devices.push(device.clone());
    save_devices(home, &devices)?;
    Ok((token, device))
}

/// A pairing code: `factory://?t=<token>&name=<this machine>&e=<endpoint
/// id>`, then `&r=<relay URL>` for each relay and `&a=<ip:port>` for each
/// address it can be dialled at directly, percent-encoded.
pub fn pairing_code(token: &str, endpoint_id: &str, relays: &[String], addresses: &[SocketAddr]) -> String {
    let mut code = format!("factory://?t={token}&name={}&e={endpoint_id}", encode(machine_name()));
    for relay in relays {
        code.push_str(&format!("&r={}", encode(relay)));
    }
    for address in addresses {
        code.push_str(&format!("&a={}", encode(&address.to_string())));
    }
    code
}

/// Pairs a device away from the running Factory (`factoryd pair`): the code
/// names its key, and config.json's own relays if it has any; a device finds
/// the rest by the key. The running Factory turns remote access on within a
/// second.
pub fn pair_offline(home: &Home, name: &str) -> Result<(String, Device), RemoteError> {
    let endpoint_id = secret_key(home)?.public().to_string();
    let relays = remote_config(home)?.map(|remote| remote.relays).unwrap_or_default();
    let (token, device) = pair(home, name)?;
    Ok((pairing_code(&token, &endpoint_id, &relays, &[]), device))
}

/// Unpairs the device with this id or name; it can't connect any more.
pub fn unpair(home: &Home, device: &str) -> Result<Device, RemoteError> {
    let _changing = devices_lock();
    let mut devices = devices(home)?;
    let Some(index) = devices.iter().position(|known| !known.local && (known.id == device || known.name == device)) else {
        return Err(RemoteError::NoDevice(device.to_owned()));
    };
    let removed = devices.remove(index);
    save_devices(home, &devices)?;
    Ok(removed)
}

fn local_pairing_path(home: &Home) -> PathBuf {
    dir(home).join("local-pairing")
}

/// The values of `name` in a pairing code.
fn code_values<'a>(code: &'a str, name: &'a str) -> impl Iterator<Item = String> + 'a {
    code.trim().split(['?', '&']).filter_map(move |part| part.strip_prefix(name)?.strip_prefix('=')).map(decode)
}

/// The loopback port the last local pairing code named, to bind again so
/// the code stays the same.
fn local_port(home: &Home) -> u16 {
    let code = std::fs::read_to_string(local_pairing_path(home)).unwrap_or_default();
    code_values(&code, "a")
        .filter_map(|address| address.parse::<SocketAddr>().ok())
        .find(|address| address.ip().is_loopback())
        .map_or(0, |address| address.port())
}

/// Writes this machine's own pairing code to `<home>/remote/local-pairing`,
/// which only this user can read: this machine's app connects with it. Its
/// token stays while the file holds it.
fn write_local_pairing(home: &Home, endpoint_id: &str, port: u16) -> Result<(), RemoteError> {
    let path = local_pairing_path(home);
    let _changing = devices_lock();
    let mut devices = devices(home)?;
    let written = std::fs::read_to_string(&path).unwrap_or_default();
    let kept = code_values(&written, "t").next();
    let local = devices.iter().position(|device| device.local);
    let token = match (kept, local) {
        (Some(token), Some(index)) if devices[index].token_sha256 == token_hash(&token) => token,
        _ => {
            let token = new_token()?;
            // A new token, but the app having connected through the tunnel
            // stays known, so a deleted file doesn't open the loopback port.
            let endpoint_id = local.map(|index| devices[index].endpoint_id.clone()).unwrap_or_default();
            let device = Device {
                id: LOCAL_DEVICE.to_owned(),
                name: machine_name().to_owned(),
                token_sha256: token_hash(&token),
                created: now_ms(),
                last_seen: None,
                last_address: String::new(),
                endpoint_id,
                code_expires: None,
                local: true,
            };
            match local {
                Some(index) => devices[index] = device,
                None => devices.push(device),
            }
            save_devices(home, &devices)?;
            token
        }
    };
    let code = pairing_code(&token, endpoint_id, &[], &[SocketAddr::from((Ipv4Addr::LOCALHOST, port))]);
    write_private(&path, format!("{code}\n").as_bytes())?;
    Ok(())
}

/// This machine's name, as devices show it ("mac-mini").
pub(crate) fn machine_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        // The system call, not the `hostname` program, which a minimal
        // Linux (NixOS, say) may not have.
        let mut buffer = [0u8; 256];
        // SAFETY: gethostname writes at most `buffer.len()` bytes into it.
        let got = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
        let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
        let name = if got { String::from_utf8_lossy(&buffer[..end]).trim().to_owned() } else { String::new() };
        let name = name.strip_suffix(".local").unwrap_or(&name).to_owned();
        if name.is_empty() { "factory".to_owned() } else { name }
    })
}

/// Percent-encodes all but letters, digits, '-', '.', '_' and '~'.
fn encode(text: &str) -> String {
    text.bytes()
        .map(
            |byte| {
                if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) { (byte as char).to_string() } else { format!("%{byte:02X}") }
            },
        )
        .collect()
}

/// Undoes `encode` (and any other percent-encoding).
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match text.get(at + 1..at + 3).filter(|_| bytes[at] == b'%').and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
            Some(byte) => {
                out.push(byte);
                at += 3;
            }
            None => {
                out.push(bytes[at]);
                at += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// How a connection travels now, and the address it comes from when it
/// comes directly.
fn path_of(connection: &Connection) -> (v1::DevicePath, Option<SocketAddr>) {
    let paths = connection.paths();
    let path = paths.iter().find(|path| path.is_selected()).or_else(|| paths.iter().next());
    match path.map(|path| path.remote_addr().clone()) {
        Some(TransportAddr::Ip(address)) => (v1::DevicePath::Direct, Some(address)),
        Some(TransportAddr::Relay(_)) => (v1::DevicePath::Relayed, None),
        Some(_) => (v1::DevicePath::Direct, None),
        None => (v1::DevicePath::None, None),
    }
}

fn from_this_machine(address: Option<SocketAddr>) -> bool {
    address.is_some_and(|address| address.ip().to_canonical().is_loopback())
}

/// Who's at the other end of a tunnel connection.
#[derive(Clone, Debug)]
pub(crate) struct Peer {
    /// Its key (iroh endpoint id), proven by the handshake.
    pub endpoint_id: String,
    /// It connects from this machine (over 127.0.0.1).
    pub loopback: bool,
    /// The address it connects from; None through a relay.
    pub address: Option<SocketAddr>,
}

/// Turns away, right after the handshake, a key that isn't a paired
/// device's, unless a code is waiting for its device or it comes from this
/// machine (whose app proves itself with the local pairing code's token).
#[derive(Debug)]
struct Gate {
    home: Home,
}

impl Gate {
    fn admits(&self, connection: &Connection) -> bool {
        if from_this_machine(path_of(connection).1) {
            return true;
        }
        let id = connection.remote_id().to_string();
        let now = now_ms();
        match devices(&self.home) {
            Ok(devices) => {
                devices.iter().any(|device| (!device.endpoint_id.is_empty() && device.endpoint_id == id) || device.awaiting(now))
            }
            Err(error) => {
                tracing::error!(%error, "could not read the paired devices");
                false
            }
        }
    }
}

impl EndpointHooks for Gate {
    fn after_handshake<'a>(&'a self, connection: &'a Connection) -> impl Future<Output = AfterHandshakeOutcome> + Send + 'a {
        let admitted = self.admits(connection);
        if !admitted {
            tracing::warn!(endpoint = %connection.remote_id(), "turned away a device that isn't paired");
        }
        async move {
            if admitted {
                AfterHandshakeOutcome::Accept
            } else {
                AfterHandshakeOutcome::Reject { error_code: REFUSED.into(), reason: b"this device isn't paired with this machine".to_vec() }
            }
        }
    }
}

/// What the tunnel should be.
#[derive(Clone, Debug, PartialEq)]
struct Wanted {
    /// Remote access: on every address, with relays; else 127.0.0.1 only.
    on: bool,
    /// config.json's relays; empty for the default ones.
    relays: Vec<String>,
}

#[derive(Default)]
struct Tunnel {
    running: Option<(Wanted, Endpoint, tokio::task::JoinHandle<()>)>,
    /// What failed to start last, so it isn't tried every second.
    failed: Option<Wanted>,
}

/// One of a device's open sessions.
struct Session {
    closed: Arc<tokio::sync::Notify>,
    endpoint_id: String,
}

/// What Factory keeps of remote access while it runs.
#[derive(Default)]
pub(crate) struct State {
    shown: Mutex<Shown>,
    /// The open sessions of each device, by its id.
    connected: Mutex<HashMap<String, Vec<Session>>>,
    /// When each key last gave a wrong token.
    failures: Mutex<HashMap<String, Instant>>,
    /// The open tunnel connections of each key, numbered.
    links: Mutex<HashMap<String, Vec<(u64, Connection)>>>,
    next_link: AtomicU64,
    /// The running endpoint.
    endpoint: Mutex<Option<Endpoint>>,
    tunnel: tokio::sync::Mutex<Tunnel>,
    /// How long making a code waits for the relay; tests make it shorter.
    online_wait: Mutex<Option<Duration>>,
}

impl State {
    /// Knows Factory's endpoint id from the start, so clients see it with
    /// the first Workspace rather than in a change just after.
    pub(crate) fn new(home: &Home) -> State {
        let state = State::default();
        match secret_key(home) {
            Ok(key) => lock(&state.shown).endpoint_id = key.public().to_string(),
            Err(error) => tracing::error!(%error, "could not read the tunnel's key"),
        }
        state
    }
}

/// What the Workspace shows of remote access.
#[derive(Clone, Default, PartialEq)]
struct Shown {
    devices: Vec<Device>,
    on: bool,
    error: String,
    endpoint_id: String,
    relays: Vec<String>,
    custom_relays: bool,
    home_relay: String,
    relay_connected: bool,
    /// How each connected device's connection travels, by its id.
    paths: HashMap<String, v1::DevicePath>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// n0's relays, which the tunnel uses unless config.json names others.
fn default_relays() -> Vec<String> {
    RelayMode::Default.relay_map().urls::<Vec<RelayUrl>>().iter().map(ToString::to_string).collect()
}

/// One of a device's sessions; it counts as connected while this lives.
pub(crate) struct Connected {
    pub device: Device,
    /// Notified when the device is unpaired: the session closes.
    pub closed: Arc<tokio::sync::Notify>,
    factory: Weak<Factory>,
}

impl Drop for Connected {
    fn drop(&mut self) {
        let Some(factory) = self.factory.upgrade() else { return };
        {
            let mut connected = lock(&factory.remote.connected);
            if let Some(sessions) = connected.get_mut(&self.device.id) {
                sessions.retain(|session| !Arc::ptr_eq(&session.closed, &self.closed));
                if sessions.is_empty() {
                    connected.remove(&self.device.id);
                }
            }
        }
        factory.broadcast_workspace(&factory.lock());
    }
}

impl Factory {
    /// The paired device whose token this is, if `peer` may use it: the key
    /// bound to it, or the first key to use its code; for this machine's app,
    /// any key from this machine. Connected until the returned `Connected`
    /// goes. A wrong token is answered no sooner than a second after the
    /// key's last wrong one.
    pub(crate) async fn authorize(self: &Arc<Self>, token: &str, peer: &Peer) -> Option<Connected> {
        let digest = token_hash(token);
        let now = now_ms();
        let mut found = None;
        let changing = devices_lock();
        match devices(&self.home) {
            Ok(mut devices) => {
                for device in &mut devices {
                    // Compared in full every time, whatever matches.
                    let known = device.token_sha256.as_bytes();
                    let same =
                        known.len() == digest.len() && known.iter().zip(digest.as_bytes()).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0;
                    // On the loopback port only this machine's app's token counts.
                    if !same || token.is_empty() || (peer.endpoint_id.is_empty() && !device.local) {
                        continue;
                    }
                    let allowed = if device.local {
                        peer.loopback || device.endpoint_id == peer.endpoint_id
                    } else if !device.endpoint_id.is_empty() {
                        device.endpoint_id == peer.endpoint_id
                    } else if device.awaiting(now) {
                        device.code_expires = None;
                        true
                    } else {
                        false
                    };
                    if allowed {
                        if !peer.endpoint_id.is_empty() {
                            device.endpoint_id.clone_from(&peer.endpoint_id);
                        }
                        device.last_seen = Some(now);
                        device.last_address = peer.address.map(|address| address.ip().to_canonical().to_string()).unwrap_or_default();
                        found = Some(device.clone());
                    }
                }
                if found.is_some()
                    && let Err(error) = save_devices(&self.home, &devices)
                {
                    tracing::warn!(%error, "could not note when a device connected");
                }
            }
            Err(error) => tracing::error!(%error, "could not read the paired devices"),
        }
        drop(changing);
        let Some(device) = found else {
            let wait = {
                let now = Instant::now();
                let last = lock(&self.remote.failures).insert(peer.endpoint_id.clone(), now);
                last.map(|last| FAILURE_PAUSE.saturating_sub(now.duration_since(last))).unwrap_or_default()
            };
            tracing::warn!(endpoint = %peer.endpoint_id, "refused a device that isn't paired");
            tokio::time::sleep(wait).await;
            return None;
        };
        let closed = Arc::new(tokio::sync::Notify::new());
        let session = Session { closed: closed.clone(), endpoint_id: peer.endpoint_id.clone() };
        lock(&self.remote.connected).entry(device.id.clone()).or_default().push(session);
        self.refresh_remote();
        Some(Connected { device, closed, factory: Arc::downgrade(self) })
    }

    /// The loopback port lets in a client with no token: only while this
    /// machine's app has never connected through the tunnel.
    pub(crate) fn loopback_open(&self) -> bool {
        match devices(&self.home) {
            Ok(devices) => !devices.iter().any(|device| device.local && !device.endpoint_id.is_empty()),
            Err(error) => {
                tracing::error!(%error, "could not read the paired devices");
                false
            }
        }
    }

    /// Closes a device's sessions and its tunnel connections.
    fn close_device(&self, id: &str, endpoint_id: &str) {
        let sessions = lock(&self.remote.connected).remove(id).unwrap_or_default();
        let mut keys: Vec<&str> = sessions.iter().map(|session| session.endpoint_id.as_str()).collect();
        keys.push(endpoint_id);
        keys.retain(|key| !key.is_empty());
        keys.sort_unstable();
        keys.dedup();
        for session in &sessions {
            session.closed.notify_one();
        }
        let links = lock(&self.remote.links);
        for key in keys {
            for (_, connection) in links.get(key).into_iter().flatten() {
                connection.close(UNPAIRED.into(), b"this device was unpaired");
            }
        }
    }

    /// Re-reads what the Workspace shows of remote access, and sends it if
    /// it changed. Devices unpaired meanwhile (by `factoryd unpair`) are
    /// disconnected.
    pub(crate) fn refresh_remote(&self) {
        let now = now_ms();
        let devices = devices(&self.home).unwrap_or_else(|error| {
            tracing::error!(%error, "could not read the paired devices");
            Vec::new()
        });
        let gone: Vec<String> =
            lock(&self.remote.connected).keys().filter(|id| !devices.iter().any(|device| &device.id == *id)).cloned().collect();
        for id in gone {
            self.close_device(&id, "");
        }
        let relays = self.lock().config.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default();
        let on = remote_on(&devices, now);
        let endpoint = lock(&self.remote.endpoint).clone();
        let (home_relay, relay_connected) = match &endpoint {
            Some(endpoint) if on => {
                let statuses = endpoint.home_relay_status().get();
                statuses.first().map(|status| (status.url().to_string(), status.is_connected())).unwrap_or_default()
            }
            _ => (String::new(), false),
        };
        let paths = {
            let links = lock(&self.remote.links);
            devices
                .iter()
                .filter(|device| !device.endpoint_id.is_empty())
                .filter_map(|device| {
                    let (_, open) = links.get(&device.endpoint_id)?.iter().find(|(_, connection)| connection.close_reason().is_none())?;
                    Some((device.id.clone(), path_of(open).0))
                })
                .collect()
        };
        let changed = {
            let mut shown = lock(&self.remote.shown);
            let now = Shown {
                // This machine's app isn't one of the devices shown.
                devices: devices.into_iter().filter(|device| !device.local).collect(),
                on,
                relays: match (on, relays.is_empty()) {
                    (false, _) => Vec::new(),
                    (true, true) => default_relays(),
                    (true, false) => relays.clone(),
                },
                custom_relays: !relays.is_empty(),
                home_relay,
                relay_connected,
                paths,
                ..shown.clone()
            };
            let changed = now != *shown;
            *shown = now;
            changed
        };
        if changed {
            self.broadcast_workspace(&self.lock());
        }
    }

    fn set_remote_shown(&self, change: impl FnOnce(&mut Shown)) {
        let changed = {
            let mut shown = lock(&self.remote.shown);
            let before = shown.clone();
            change(&mut shown);
            before != *shown
        };
        if changed {
            self.broadcast_workspace(&self.lock());
        }
    }

    /// The Workspace's remote access.
    pub(crate) fn remote_view(&self, _inner: &Inner) -> v1::RemoteAccess {
        let shown = lock(&self.remote.shown).clone();
        let connected = lock(&self.remote.connected);
        v1::RemoteAccess {
            on: shown.on,
            port: 0,
            listen: String::new(),
            error: shown.error,
            fingerprint: String::new(),
            addresses: Vec::new(),
            devices: shown
                .devices
                .into_iter()
                .filter(|device| !device.local)
                .map(|device| {
                    let connected = connected.contains_key(&device.id);
                    let path =
                        if connected { shown.paths.get(&device.id).copied().unwrap_or(v1::DevicePath::None) } else { v1::DevicePath::None };
                    v1::PairedDevice {
                        connected,
                        path: path.into(),
                        // A device that never bound a key and has no code
                        // (paired before the tunnel) shows as expired.
                        code_expires_ms: if device.endpoint_id.is_empty() { device.code_expires.unwrap_or(device.created) } else { 0 },
                        id: device.id,
                        name: device.name,
                        paired_ms: device.created,
                        last_seen_ms: device.last_seen.unwrap_or_default(),
                        last_address: device.last_address,
                    }
                })
                .collect(),
            machine: machine_name().to_owned(),
            endpoint_id: shown.endpoint_id,
            relays: shown.relays,
            custom_relays: shown.custom_relays,
            home_relay: shown.home_relay,
            relay_connected: shown.relay_connected,
        }
    }

    /// How long making a pairing code waits for the tunnel to reach its
    /// relay (5 s); tests make it shorter.
    pub fn set_tunnel_online_wait(&self, wait: Duration) {
        *lock(&self.remote.online_wait) = Some(wait);
    }

    /// Pairs a new device, from the app: remote access turns on, and the
    /// code names the relay the tunnel reaches and the addresses it can be
    /// dialled at.
    pub async fn create_pairing(self: &Arc<Self>, name: &str) -> Result<v1::PairingCode, RequestError> {
        let (home, name) = (self.home.clone(), name.to_owned());
        let paired = tokio::task::spawn_blocking(move || pair(&home, &name)).await.map_err(std::io::Error::other)?;
        let (token, device) = paired.map_err(|error| RequestError::Remote(error.to_string()))?;
        sync_tunnel(self).await;
        self.refresh_remote();
        let Some(endpoint) = lock(&self.remote.endpoint).clone() else {
            let error = lock(&self.remote.shown).error.clone();
            return Err(RequestError::Remote(if error.is_empty() { "The private tunnel isn't running.".to_owned() } else { error }));
        };
        let wait = lock(&self.remote.online_wait).unwrap_or(ONLINE_WAIT);
        let _ = tokio::time::timeout(wait, endpoint.online()).await;
        let addr = endpoint.addr();
        let mut relays: Vec<String> = addr.relay_urls().map(ToString::to_string).collect();
        for relay in self.lock().config.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default() {
            if !relays.iter().any(|known| known.trim_end_matches('/') == relay.trim_end_matches('/')) {
                relays.push(relay);
            }
        }
        let addresses: Vec<SocketAddr> = addr.ip_addrs().filter(|address| !address.ip().is_loopback()).copied().collect();
        let code = pairing_code(&token, &endpoint.id().to_string(), &relays, &addresses);
        Ok(v1::PairingCode { code, device_id: device.id })
    }

    /// The folders in a folder of this machine, for an app on another one
    /// to pick a project's folder by browsing rather than typing its path.
    /// `path` is absolute, or starts at the home folder ("", "~" or "~/…").
    pub async fn list_folders(&self, path: &str) -> Result<v1::FolderList, RequestError> {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || list_folders(&path)).await.map_err(std::io::Error::other)?.map_err(RequestError::Remote)
    }

    /// Unpairs a device, from the app: its connections close. Unpairing the
    /// last one turns remote access off.
    pub async fn unpair_device(&self, id: &str) -> Result<(), RequestError> {
        let removed = match unpair(&self.home, id) {
            Ok(removed) => removed,
            Err(RemoteError::NoDevice(id)) => return Err(RequestError::UnknownDevice(id)),
            Err(error) => return Err(RequestError::Remote(error.to_string())),
        };
        self.close_device(&removed.id, &removed.endpoint_id);
        self.refresh_remote();
        Ok(())
    }

    /// Sets the relays the tunnel uses, in config.json's `remote.relays`;
    /// none for the default ones.
    pub async fn set_relays(&self, relays: &[String]) -> Result<(), RequestError> {
        let relays: Vec<String> = relays.iter().map(|relay| relay.trim().to_owned()).filter(|relay| !relay.is_empty()).collect();
        self.edit_config(|config| {
            let mut remote = config.remote.take().unwrap_or_default();
            remote.relays = relays;
            config.remote = (remote != RemoteConfig::default()).then_some(remote);
            Ok(())
        })
        .await
    }
}

/// A folder listing shows at most this many folders.
const MAX_FOLDERS: usize = 5_000;

fn list_folders(path: &str) -> Result<v1::FolderList, String> {
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let path = path.trim();
    let wanted = match path {
        "" | "~" => home.clone(),
        _ => match path.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(path),
        },
    };
    if !wanted.is_absolute() {
        return Err(format!("{path} isn't a full path: start it with / or ~"));
    }
    let folder = std::fs::canonicalize(&wanted).map_err(|error| format!("Can't open {}: {}", wanted.display(), plain_error(&error)))?;
    if !folder.is_dir() {
        return Err(format!("{} isn't a folder", folder.display()));
    }
    let entries = std::fs::read_dir(&folder).map_err(|error| format!("Can't read {}: {}", folder.display(), plain_error(&error)))?;
    let mut folders: Vec<v1::FolderEntry> = entries
        .filter_map(Result::ok)
        // Links to folders count; what can't be looked at is left out.
        .filter(|entry| std::fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir()))
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            v1::FolderEntry { hidden: name.starts_with('.'), name }
        })
        .collect();
    folders.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.name.cmp(&b.name)));
    let truncated = folders.len() > MAX_FOLDERS;
    folders.truncate(MAX_FOLDERS);
    Ok(v1::FolderList {
        parent: folder.parent().map(|parent| parent.display().to_string()).unwrap_or_default(),
        path: folder.display().to_string(),
        folders,
        home: home.display().to_string(),
        truncated,
    })
}

/// "permission denied", not "Permission denied (os error 13)".
fn plain_error(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "it doesn't exist".to_owned(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_owned(),
        _ => error.to_string(),
    }
}

/// Keeps the tunnel as it should be: on 127.0.0.1 only, or on every address
/// with relays while a device on another machine can connect, rebuilt within
/// a second of a change. Also keeps what the Workspace shows of it current.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    while let Some(strong) = factory.upgrade() {
        sync_tunnel(&strong).await;
        strong.refresh_remote();
        drop(strong);
        tokio::time::sleep(CHECK).await;
    }
}

/// Starts the tunnel, or starts it again when it should change. Boxed: a
/// client the tunnel serves can pair a device, which comes back here.
fn sync_tunnel(factory: &Arc<Factory>) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
    Box::pin(async move {
        let devices = devices(&factory.home).unwrap_or_else(|error| {
            tracing::error!(%error, "could not read the paired devices");
            Vec::new()
        });
        let relays = factory.lock().config.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default();
        let wanted = Wanted { on: remote_on(&devices, now_ms()), relays };
        let mut tunnel = factory.remote.tunnel.lock().await;
        if tunnel.running.as_ref().is_some_and(|(running, ..)| *running == wanted) || tunnel.failed.as_ref() == Some(&wanted) {
            return;
        }
        let mut port = local_port(&factory.home);
        if let Some((_, endpoint, accept)) = tunnel.running.take() {
            accept.abort();
            port = loopback_port(&endpoint).unwrap_or(port);
            lock(&factory.remote.endpoint).take();
            endpoint.close().await;
        }
        let home = factory.home.clone();
        let started = async {
            let key = {
                let home = home.clone();
                tokio::task::spawn_blocking(move || secret_key(&home)).await.map_err(std::io::Error::other)??
            };
            let endpoint = bind(&home, key, &wanted, port).await?;
            let (id, port) = (endpoint.id().to_string(), loopback_port(&endpoint).unwrap_or_default());
            let written =
                tokio::task::spawn_blocking(move || write_local_pairing(&home, &id, port)).await.map_err(std::io::Error::other)?;
            if let Err(error) = written {
                endpoint.close().await;
                return Err(error);
            }
            Ok::<_, RemoteError>(endpoint)
        }
        .await;
        match started {
            Ok(endpoint) => {
                tracing::info!(remote_access = wanted.on, endpoint = %endpoint.id(), "private tunnel started");
                let accept = tokio::spawn(accept(endpoint.clone(), Arc::downgrade(factory)));
                *lock(&factory.remote.endpoint) = Some(endpoint.clone());
                let id = endpoint.id().to_string();
                tunnel.running = Some((wanted, endpoint, accept));
                tunnel.failed = None;
                factory.set_remote_shown(|shown| {
                    shown.error.clear();
                    shown.endpoint_id = id;
                });
            }
            Err(error) => {
                tracing::error!(%error, "could not start the private tunnel");
                tunnel.failed = Some(wanted);
                factory.set_remote_shown(|shown| shown.error = error.to_string());
            }
        }
    })
}

/// The port the endpoint answers on at 127.0.0.1.
fn loopback_port(endpoint: &Endpoint) -> Option<u16> {
    endpoint.bound_sockets().into_iter().find(SocketAddr::is_ipv4).map(|address| address.port())
}

/// An endpoint as `wanted` says, on `port` (the local pairing code's) if it's
/// free. Without remote access: 127.0.0.1 only, no relays, nothing published.
/// With it: every address, the relays, and n0's address lookup with the
/// default relays (with config.json's own relays, nothing goes to n0).
/// Never a port mapping (no portmapper).
async fn bind(home: &Home, key: SecretKey, wanted: &Wanted, port: u16) -> Result<Endpoint, RemoteError> {
    let failed = |error: &dyn std::fmt::Display| RemoteError::Tunnel(error.to_string());
    let builder = |port: u16| -> Result<Builder, RemoteError> {
        let builder =
            if wanted.on && wanted.relays.is_empty() { Endpoint::builder(presets::N0) } else { Endpoint::builder(presets::Minimal) };
        let relay_mode = if !wanted.on {
            RelayMode::Disabled
        } else if wanted.relays.is_empty() {
            RelayMode::Default
        } else {
            let urls = wanted
                .relays
                .iter()
                .map(|relay| relay.parse::<RelayUrl>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| failed(&error))?;
            RelayMode::Custom(urls.into_iter().collect::<RelayMap>())
        };
        let ip: IpAddr = if wanted.on { Ipv4Addr::UNSPECIFIED.into() } else { Ipv4Addr::LOCALHOST.into() };
        let mut builder = builder
            .secret_key(key.clone())
            .alpns(vec![ALPN.to_vec()])
            .hooks(Gate { home: home.clone() })
            .relay_mode(relay_mode)
            .clear_ip_transports()
            .bind_addr(SocketAddr::new(ip, port))
            .map_err(|error| failed(&error))?;
        if wanted.on {
            let optional = BindOpts::default().set_is_required(false);
            builder =
                builder.bind_addr_with_opts(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)), optional).map_err(|error| failed(&error))?;
        }
        Ok(builder)
    };
    // The endpoint before may take a moment to let go of the port.
    let mut tries = 0;
    loop {
        match builder(port)?.bind().await {
            Ok(endpoint) => return Ok(endpoint),
            Err(_) if port != 0 && tries < 20 => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) if port != 0 => {
                tracing::info!(port, %error, "the tunnel's port is taken; it takes another");
                return builder(0)?.bind().await.map_err(|error| failed(&error));
            }
            Err(error) => return Err(failed(&error)),
        }
    }
}

/// Serves each connection the gate let in until the endpoint closes.
async fn accept(endpoint: Endpoint, factory: Weak<Factory>) {
    while let Some(incoming) = endpoint.accept().await {
        let factory = factory.clone();
        tokio::spawn(async move {
            match incoming.await {
                Ok(connection) => serve_link(connection, factory).await,
                Err(error) => tracing::debug!(%error, "a tunnel connection didn't open"),
            }
        });
    }
}

/// One device's connection: each stream it opens is a client, served as
/// the loopback port serves one, until the connection ends.
async fn serve_link(connection: Connection, factory: Weak<Factory>) {
    let endpoint_id = connection.remote_id().to_string();
    let number = {
        let Some(strong) = factory.upgrade() else { return };
        let number = strong.remote.next_link.fetch_add(1, Ordering::Relaxed);
        lock(&strong.remote.links).entry(endpoint_id.clone()).or_default().push((number, connection.clone()));
        number
    };
    while let Ok((send, receive)) = connection.accept_bi().await {
        let Some(strong) = factory.upgrade() else { break };
        let address = path_of(&connection).1;
        let peer = Peer { endpoint_id: endpoint_id.clone(), loopback: from_this_machine(address), address };
        tokio::spawn(async move {
            if let Err(error) = crate::connection::serve_stream(tokio::io::join(receive, send), strong, Some(peer)).await {
                tracing::warn!(%error, "tunnel connection failed");
            }
        });
    }
    if let Some(strong) = factory.upgrade() {
        let mut links = lock(&strong.remote.links);
        if let Some(open) = links.get_mut(&endpoint_id) {
            open.retain(|(known, _)| *known != number);
            if open.is_empty() {
                links.remove(&endpoint_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_in_pairing_codes_are_percent_encoded() {
        assert_eq!(encode("Mac mini/2"), "Mac%20mini%2F2");
        assert_eq!(encode("box-1.lan"), "box-1.lan");
        assert_eq!(decode("Mac%20mini%2F2"), "Mac mini/2");
        assert_eq!(decode("100%"), "100%");
        let code = pairing_code("ab", "id", &["https://relay.example/".into()], &["127.0.0.1:7".parse().unwrap()]);
        assert_eq!(code_values(&code, "r").collect::<Vec<_>>(), ["https://relay.example/"]);
        assert_eq!(code_values(&code, "a").collect::<Vec<_>>(), ["127.0.0.1:7"]);
        assert_eq!(code_values(&code, "t").collect::<Vec<_>>(), ["ab"]);
    }

    #[test]
    fn remote_access_is_on_while_a_device_can_connect() {
        let device = |endpoint_id: &str, code_expires: Option<i64>, local: bool| Device {
            id: "d".into(),
            name: "D".into(),
            token_sha256: String::new(),
            created: 0,
            last_seen: None,
            last_address: String::new(),
            endpoint_id: endpoint_id.into(),
            code_expires,
            local,
        };
        assert!(!remote_on(&[], 100));
        assert!(!remote_on(&[device("k", None, true)], 100), "this machine's app alone");
        assert!(remote_on(&[device("", Some(200), false)], 100), "a code waiting");
        assert!(!remote_on(&[device("", Some(50), false)], 100), "a code expired");
        assert!(!remote_on(&[device("", None, false)], 100), "paired before the tunnel");
        assert!(remote_on(&[device("k", None, false)], 100), "a device with its key");
    }
}
