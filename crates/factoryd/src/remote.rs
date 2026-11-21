//! Devices reach Factory through its private tunnel (iroh): an endpoint with
//! a key of its own (`<home>/remote/iroh.key`), dialled by that key, end to
//! end encrypted, directly or through a relay that can't read it. Factory
//! listens on no port for them. Each app connection is one bidirectional
//! stream on ALPN `claktory/1`, carrying a WebSocket and then the protocol,
//! as on the loopback port; on `claktory/2`, the same bytes compressed
//! (`compressed`), for apps that offer it.
//!
//! - Each paired device has a token, given once in its pairing code; Factory
//!   keeps only the token's SHA-256 (`<home>/remote/devices.json`), read
//!   afresh at every handshake, so `factoryd unpair` applies at once.
//! - The first device to connect with a code claims it: no other key may
//!   use it, and it waits, its Hello answered with PairingPending, until the
//!   person allows it on a device that's already in (or `factoryd pair` in a
//!   terminal); then its key is bound to the code. Both show the same four
//!   digits (`approval_number`), so the person can tell their device from
//!   one that copied the code. A code lasts only while the person pairs:
//!   turning the device away, cancelling (Add a device closed) or its ten
//!   minutes running out ends it, and its device is forgotten.
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
//! The loopback port (7420) wants the local pairing code's token too, always:
//! any user of this machine can reach the port, but only this one can read
//! the code (`<home>` and `<home>/remote` are 0700, the file 0600). The code
//! is written when Factory opens, before the port serves anyone, so this
//! machine's app finds it even when the tunnel can't start. (An agent of
//! this user can read it too: a bar, not a boundary.)

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use factory_protocol::v1;
use iroh::endpoint::{AfterHandshakeOutcome, BindOpts, Builder, Connection, EndpointHooks, presets};
use iroh::{Endpoint, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{Config, RemoteConfig};
use crate::encoding::{hex, unhex};
use crate::factory::{Factory, Inner, RequestError};
use crate::home::Home;
use crate::transcript::now_ms;

/// The backend's protocol on iroh, the same in the app (app/rust).
pub const ALPN: &[u8] = b"claktory/1";
/// The same, compressed (`compressed`): what apps that have it offer first.
pub const ALPN_COMPRESSED: &[u8] = b"claktory/2";
/// A paired device reaching this machine's localhost (`crate::forward`).
pub const ALPN_FORWARD: &[u8] = b"claktory-forward/1";
/// How hard `claktory/2` compresses: zstd's default, fast enough that a
/// transcript of megabytes costs milliseconds.
const COMPRESSION: async_compression::Level = async_compression::Level::Default;
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
    #[error("no device named \"{0}\" is waiting to be allowed")]
    NotWaiting(String),
    #[error("{0} isn't a key Claktory can read: delete it, and Claktory makes a new one (devices then need pairing again)")]
    BadKey(PathBuf),
    #[error("the private tunnel can't start: {0}")]
    Tunnel(String),
}

/// Where Factory keeps what devices need.
fn dir(home: &Home) -> PathBuf {
    home.root().join("remote")
}

/// Makes `path` a folder only this user can open, as `<home>` and
/// `<home>/remote` are: what other users of this machine could read or
/// change there is removed from its permissions.
pub(crate) fn private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path)?;
    let mode = std::fs::metadata(path)?.permissions().mode();
    if mode & 0o077 != 0 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o700))?;
    }
    Ok(())
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
            private_dir(&dir(home))?;
            write_private(&path, format!("{}\n", hex(&key.to_bytes())).as_bytes())?;
            Ok(key)
        }
        Err(error) => Err(error.into()),
    }
}

/// A device paired with this Factory.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The key of the first device to use its code, waiting for the person
    /// to allow it; empty until then, and once it's allowed (its key is
    /// then `endpoint_id`). Kept apart from `endpoint_id` so a backend from
    /// before approvals, after a rollback, takes the code as unused rather
    /// than letting that device in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pending_endpoint_id: String,
    /// What the waiting device says it is (a `v1::DeviceKind`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pending_kind: i32,
    /// When it asked, in ms since 1970.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_since: Option<i64>,
    /// Its code was made without a name: the first device to use it gives
    /// its own (`Hello.device_name`). False once it has.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unnamed: bool,
    /// It came without a code, found through the person's account: it
    /// connects by its key alone, and its token is the SHA-256 of the
    /// assertion it first showed (for the four digits), never a code's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub discovered: bool,
    /// The account it's confirmed as the owner's for: it showed an
    /// assertion for it, or the person confirmed it from a confirmed
    /// device. Empty if neither.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub confirmed_as: String,
    /// When its grace to confirm ends, in ms since 1970: set at its first
    /// Hello from an app that can confirm while this Claktory is signed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_by: Option<i64>,
    /// The device waiting with it showed a valid assertion for this
    /// Claktory's account.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending_signed_in: bool,
}

/// What a device made without a name is called until one uses its code.
const UNNAMED: &str = "New device";
/// The longest name a device takes for itself.
const NAME_CHARS: usize = 60;

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_zero(value: &i32) -> bool {
    *value == 0
}

impl Device {
    /// Its code is waiting for its device: not used yet and not expired.
    /// It may have a device waiting to be allowed.
    pub(crate) fn awaiting(&self, now: i64) -> bool {
        !self.local && self.endpoint_id.is_empty() && self.code_expires.is_some_and(|expires| expires > now)
    }

    /// A device used its code and waits for the person to allow it.
    pub fn is_pending(&self, now: i64) -> bool {
        self.awaiting(now) && !self.pending_endpoint_id.is_empty()
    }

    /// Its code ran out before a device was allowed with it.
    pub(crate) fn expired(&self, now: i64) -> bool {
        !self.local && self.endpoint_id.is_empty() && self.code_expires.is_some_and(|expires| expires <= now)
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
    private_dir(&dir(home))?;
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

/// An id for a new device called `name` (trimmed), unused in `devices`,
/// and the name it shows (its id's base when `name` has nothing to show).
fn new_id(devices: &[Device], name: &str) -> (String, String) {
    let base: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let base = base.trim_matches('-');
    let base = if base.is_empty() || base == LOCAL_DEVICE { "device" } else { base };
    let mut id = base.to_owned();
    let mut n = 2;
    while devices.iter().any(|device| device.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    (id, if name.is_empty() { base.to_owned() } else { name.to_owned() })
}

/// Pairs a new device: makes its token, good for `CODE_LIFETIME` until the
/// device first connects. Returns the token, which its pairing code holds,
/// so it's shown once and never again, and the device.
pub fn pair(home: &Home, name: &str) -> Result<(String, Device), RemoteError> {
    let token = new_token()?;
    let _changing = devices_lock();
    let mut devices = devices(home)?;
    // Without a name, the device that uses the code names itself.
    let unnamed = name.trim().is_empty();
    let name = if unnamed { UNNAMED } else { name.trim() };
    let (id, name) = new_id(&devices, name);
    let now = now_ms();
    let device = Device {
        id,
        name,
        token_sha256: token_hash(&token),
        created: now,
        last_seen: None,
        last_address: String::new(),
        endpoint_id: String::new(),
        code_expires: Some(now + CODE_LIFETIME.as_millis() as i64),
        local: false,
        pending_endpoint_id: String::new(),
        pending_kind: 0,
        pending_since: None,
        unnamed,
        ..Device::default()
    };
    devices.push(device.clone());
    save_devices(home, &devices)?;
    Ok((token, device))
}

/// A pairing code: `factory://?t=<token>&name=<this Claktory's
/// name>&e=<endpoint id>&color=<its color>`, then `&r=<relay URL>` for each
/// relay and `&a=<ip:port>` for each address it can be dialled at directly,
/// percent-encoded.
pub fn pairing_code(token: &str, endpoint_id: &str, relays: &[String], addresses: &[SocketAddr], claktory: &Identity) -> String {
    let mut code = format!("factory://?t={token}&name={}&e={endpoint_id}&color={}", encode(&claktory.name), encode(&claktory.color));
    for relay in relays {
        code.push_str(&format!("&r={}", encode(relay)));
    }
    for address in addresses {
        code.push_str(&format!("&a={}", encode(&address.to_string())));
    }
    code
}

/// What a Claktory is called and its color, as devices show it.
pub struct Identity {
    pub name: String,
    pub color: String,
}

/// This Claktory's identity from config.json, away from the running Factory
/// (defaults when the file is missing or broken).
fn identity(home: &Home) -> Identity {
    let config = std::fs::read_to_string(home.config_path()).ok().and_then(|text| Config::parse(&text).ok()).unwrap_or_default();
    Identity { name: config.claktory_name(), color: config.claktory_color(key_color(home)).to_owned() }
}

/// The color a Claktory has until the person picks one: one of the agents'
/// palette, chosen from its key, so it stays the same and differs between
/// Claktories without being written anywhere.
pub(crate) fn key_color(home: &Home) -> &'static str {
    let index = secret_key(home).map_or(0, |key| usize::from(Sha256::digest(key.public().as_bytes())[0]));
    crate::config::AGENT_COLORS[index % crate::config::AGENT_COLORS.len()]
}

/// The seed of the person's avatar until they shuffle it: from this
/// Claktory's key, as its color is, so it stays the same without being
/// written anywhere.
pub(crate) fn key_seed(home: &Home) -> String {
    secret_key(home).map(|key| key.public().to_string()).unwrap_or_default()
}

/// Pairs a device away from the running Factory (`factoryd pair`): the code
/// names its key, and config.json's own relays if it has any; a device finds
/// the rest by the key. The running Factory turns remote access on within a
/// second.
pub fn pair_offline(home: &Home, name: &str) -> Result<(String, Device), RemoteError> {
    let endpoint_id = secret_key(home)?.public().to_string();
    let relays = remote_config(home)?.map(|remote| remote.relays).unwrap_or_default();
    let (token, device) = pair(home, name)?;
    Ok((pairing_code(&token, &endpoint_id, &relays, &[], &identity(home)), device))
}

/// A new token for the device `id` made at `created`, while its code waits
/// for a device and none has used it: what an agent's pairing shows after a
/// restart, which forgot the token. The code made with the old one stops
/// working. Err(NotWaiting) otherwise.
fn rekey(home: &Home, id: &str, created: i64) -> Result<String, RemoteError> {
    let token = new_token()?;
    let _changing = devices_lock();
    let mut devices = devices(home)?;
    let now = now_ms();
    let Some(device) = devices
        .iter_mut()
        .find(|device| device.id == id && device.created == created && device.awaiting(now) && device.pending_endpoint_id.is_empty())
    else {
        return Err(RemoteError::NotWaiting(id.to_owned()));
    };
    device.token_sha256 = token_hash(&token);
    save_devices(home, &devices)?;
    Ok(token)
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

/// The four digits both the person's device and the new one show while it
/// waits to be allowed, from both ends of its handshake and its code: see
/// `v1::PendingDevice.number`. A device that copied the code has a key of
/// its own, so it shows other digits than the one the person is pairing.
pub fn approval_number(claktory: &str, device: &str, token_sha256: &str) -> String {
    let digest = Sha256::digest(format!("claktory pairing\n{claktory}\n{device}\n{token_sha256}").as_bytes());
    let number = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 10_000;
    format!("{number:04}")
}

/// How a code that let no device in ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// The person turned the device away.
    Denied,
    /// The person closed Add a device, or `factoryd pair` was stopped.
    Cancelled,
    /// Its time ran out.
    Expired,
}

/// Changes a device that hasn't been let in yet: `Some(Ended)` removes it
/// (only one not allowed yet), None allows the device waiting with its code.
/// Err(NotWaiting) when there's none so (for Denied and allowing, one that
/// isn't waiting).
fn settle(home: &Home, device: &str, ended: Option<Ended>) -> Result<Device, RemoteError> {
    let _changing = devices_lock();
    let mut devices = devices(home)?;
    let now = now_ms();
    let waiting = |known: &Device| match ended {
        None | Some(Ended::Denied) => known.is_pending(now),
        Some(_) => known.endpoint_id.is_empty() && !known.local,
    };
    let Some(index) = devices.iter().position(|known| (known.id == device || known.name == device) && waiting(known)) else {
        return Err(RemoteError::NotWaiting(device.to_owned()));
    };
    if ended.is_some() {
        let removed = devices.remove(index);
        save_devices(home, &devices)?;
        return Ok(removed);
    }
    let allowed = &mut devices[index];
    allowed.endpoint_id = std::mem::take(&mut allowed.pending_endpoint_id);
    allowed.pending_kind = 0;
    allowed.pending_since = None;
    allowed.pending_signed_in = false;
    allowed.code_expires = None;
    let allowed = allowed.clone();
    save_devices(home, &devices)?;
    Ok(allowed)
}

/// Lets in the device waiting with this device's code (by id or name).
pub fn approve(home: &Home, device: &str) -> Result<Device, RemoteError> {
    settle(home, device, None)
}

/// Turns away the device waiting with this device's code, and forgets it.
pub fn deny(home: &Home, device: &str) -> Result<Device, RemoteError> {
    settle(home, device, Some(Ended::Denied))
}

/// Ends a code that hasn't let a device in, and forgets its device.
pub fn cancel(home: &Home, device: &str) -> Result<Device, RemoteError> {
    settle(home, device, Some(Ended::Cancelled))
}

/// Forgets devices whose codes ran out before one was let in with them: a
/// code lasts only while the person pairs. Returns them.
fn forget_expired(home: &Home) -> Result<Vec<Device>, RemoteError> {
    let _changing = devices_lock();
    let mut devices = devices(home)?;
    let now = now_ms();
    let (expired, kept): (Vec<Device>, Vec<Device>) = devices.drain(..).partition(|device| device.expired(now));
    if !expired.is_empty() {
        save_devices(home, &kept)?;
    }
    Ok(expired)
}

fn local_pairing_path(home: &Home) -> PathBuf {
    dir(home).join("local-pairing")
}

/// The values of `name` in a pairing code.
pub(crate) fn code_values<'a>(code: &'a str, name: &'a str) -> impl Iterator<Item = String> + 'a {
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

/// Makes `<home>` and `<home>/remote` private and writes this machine's own
/// pairing code, before Factory serves anyone: the loopback port wants its
/// token, so this machine's app has it from its first connection on, even
/// when the tunnel can't start. Until the tunnel binds, the code names the
/// port it had before (none the first time); then it's written again.
pub(crate) fn prepare_local(home: &Home) -> Result<(), RemoteError> {
    private_dir(home.root())?;
    private_dir(&dir(home))?;
    let id = secret_key(home)?.public().to_string();
    write_local_pairing(home, &id, local_port(home))
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
            // A new token; the key this machine's app connected through the
            // tunnel with stays known.
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
                pending_endpoint_id: String::new(),
                pending_kind: 0,
                pending_since: None,
                unnamed: false,
                ..Device::default()
            };
            match local {
                Some(index) => devices[index] = device,
                None => devices.push(device),
            }
            save_devices(home, &devices)?;
            token
        }
    };
    let addresses: &[SocketAddr] = if port == 0 { &[] } else { &[SocketAddr::from((Ipv4Addr::LOCALHOST, port))] };
    let code = pairing_code(&token, endpoint_id, &[], addresses, &identity(home));
    write_private(&path, format!("{code}\n").as_bytes())?;
    Ok(())
}

/// This machine's name, as devices, the app's footer and claktory.com show
/// it: on a Mac, its Computer Name (System Settings → General → Sharing,
/// such as "Lee's Mac mini"); elsewhere, its host name without the
/// network's domain ("lees-mini", not "lees-mini.lan" or ".local").
/// `FACTORY_MACHINE_NAME` overrides it, for QA sandboxes, which must never
/// show the real machine's name (claktory-qa sets it).
pub(crate) fn machine_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        if let Some(name) = chosen_machine_name(std::env::var_os("FACTORY_MACHINE_NAME")) {
            return name;
        }
        let mac = if cfg!(target_os = "macos") {
            std::process::Command::new("/usr/sbin/scutil")
                .args(["--get", "ComputerName"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                .filter(|name| !name.is_empty() && !name.chars().any(char::is_control))
        } else {
            None
        };
        if let Some(name) = mac {
            return name;
        }
        // The system call, not the `hostname` program, which a minimal
        // Linux (NixOS, say) may not have.
        let mut buffer = [0u8; 256];
        // SAFETY: gethostname writes at most `buffer.len()` bytes into it.
        let got = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0;
        let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
        let name = if got { String::from_utf8_lossy(&buffer[..end]).trim().to_owned() } else { String::new() };
        let name = name.split('.').next().unwrap_or_default().to_owned();
        if name.is_empty() { "factory".to_owned() } else { name }
    })
}

/// The name `FACTORY_MACHINE_NAME` gives, when it's one: trimmed, not
/// empty, with no control characters, and at most 64 characters.
fn chosen_machine_name(value: Option<std::ffi::OsString>) -> Option<String> {
    let name = value?.to_string_lossy().trim().to_owned();
    (!name.is_empty() && !name.chars().any(char::is_control)).then(|| name.chars().take(64).collect::<String>().trim_end().to_owned())
}

/// Percent-encodes all but letters, digits, '-', '.', '_' and '~'.
pub(crate) fn encode(text: &str) -> String {
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
/// device's, unless a code is waiting for its device (no other key once a
/// device used it), it comes from this machine (whose app proves itself
/// with the local pairing code's token), or this Claktory is signed in to
/// an account (`open`), whose devices find it and prove themselves with an
/// assertion in their Hello.
#[derive(Debug)]
struct Gate {
    home: Home,
    /// Who may use pipes (`pipe_link`): only they get its ALPN, and only it.
    pipes: Arc<crate::pipe_link::Admits>,
    /// This Claktory is signed in: its account's devices prove themselves
    /// in their Hello, so unknown keys get past the handshake.
    open: Arc<AtomicBool>,
}

impl Gate {
    fn admits(&self, connection: &Connection) -> bool {
        // Ours: Factory dialed a pipe's peer.
        if connection.side().is_client() {
            return true;
        }
        if connection.alpn() == crate::pipe_link::ALPN_PIPE {
            return self.pipes.admits(&connection.remote_id().to_string(), now_ms());
        }
        if from_this_machine(path_of(connection).1) || self.open.load(Ordering::Relaxed) {
            return true;
        }
        let id = connection.remote_id().to_string();
        let now = now_ms();
        match devices(&self.home) {
            Ok(devices) => devices.iter().any(|device| {
                (!device.endpoint_id.is_empty() && device.endpoint_id == id)
                    || (device.awaiting(now) && (device.pending_endpoint_id.is_empty() || device.pending_endpoint_id == id))
            }),
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

/// The tunnel is two endpoints with the same key. One answers on 127.0.0.1
/// only, for this machine's app, and runs for as long as Factory does, so
/// remote access turning on or off never drops this machine's connections
/// (adding the first device from the app used to cut the app off for half
/// a minute). The other answers on every address, with relays, while
/// remote access is on.
#[derive(Default)]
struct Tunnel {
    local: Option<(Endpoint, tokio::task::JoinHandle<()>)>,
    /// With the relays it was started with.
    remote: Option<(Vec<String>, Endpoint, tokio::task::JoinHandle<()>)>,
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
    /// The endpoint devices on other machines reach, while remote access is on.
    endpoint: Mutex<Option<Endpoint>>,
    tunnel: tokio::sync::Mutex<Tunnel>,
    /// How long making a code waits for the relay; tests make it shorter.
    online_wait: Mutex<Option<Duration>>,
    /// Notified when a waiting device is allowed or its code ends.
    pairing: tokio::sync::Notify,
    /// How each code that let no device in ended, by its device's id, for
    /// the device waiting with it.
    ended: Mutex<HashMap<String, Ended>>,
    /// Connections of devices waiting to be allowed, or being told they
    /// weren't: remote access stays on for them until they've heard.
    waiting: AtomicU64,
    /// The tokens of agents' pairings still waiting for a device, by
    /// pairing id (`extensions::devices`): only in memory, never on disk.
    codes: Mutex<HashMap<String, String>>,
    /// Notified whenever devices may have changed, for agents' pairings to
    /// show it (`extensions::devices::monitor`).
    pub(crate) changed: Arc<tokio::sync::Notify>,
    /// This Claktory is signed in to an account: the gate lets in keys it
    /// doesn't know, for the person's devices to find it (see `admit`).
    account_open: Arc<AtomicBool>,
    /// The port Claktory's own listener has (7420), which devices may not
    /// forward to; 0 until it listens.
    listener_port: AtomicU64,
    /// Pipes to other Claktories, their transport's (`pipe_link`).
    pub(crate) pipes: crate::pipe_link::State,
}

impl State {
    pub(crate) fn set_listener_port(&self, port: u16) {
        self.listener_port.store(u64::from(port), Ordering::Relaxed);
    }

    pub(crate) fn listener_port(&self) -> Option<u16> {
        u16::try_from(self.listener_port.load(Ordering::Relaxed)).ok().filter(|port| *port != 0)
    }
}

/// A device's connection waiting to be allowed, while it lives.
pub(crate) struct Waiting {
    factory: Weak<Factory>,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        if let Some(factory) = self.factory.upgrade() {
            factory.remote.waiting.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// What became of a device's Hello with its token.
pub(crate) enum Authorized {
    Connected(Connected),
    /// It used a pairing code first, or came through the account, and waits
    /// for the person to allow it.
    Pending(Device),
    Refused,
    /// Turned away for a reason it can act on: the code to answer with and
    /// what to tell it.
    Turned(v1::ErrorCode, String),
}

/// What a device's Hello says of who it is (`Factory::authorize`).
pub(crate) struct Knock<'a> {
    /// Its pairing code's token, or this machine's app's; empty for a
    /// device found through the account.
    pub token: &'a str,
    /// A device assertion from the account service (Hello.assertion): it's
    /// signed in to the person's account. Empty without one.
    pub assertion: &'a str,
    /// Its app can sign in to the account and confirm (Hello.can_confirm).
    pub can_confirm: bool,
    pub kind: v1::DeviceKind,
    /// Its own name (Hello.device_name), for a device that names itself.
    pub name: &'a str,
}

/// How long a device has to confirm it's the owner's, from its first Hello
/// from an app that can while this Claktory is signed in.
pub const CONFIRM_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// How long after this Claktory signs in its first device may join through
/// the account alone, without Allow: the fresh server's case.
pub const FIRST_DEVICE_WINDOW: Duration = Duration::from_secs(30 * 60);

/// This Claktory's account, for `admit`.
pub(crate) struct Owner {
    pub account_id: String,
    /// When it signed in, in ms since 1970; 0 if before that was kept.
    pub signed_in_ms: i64,
}

/// What `admit` decided, by the device's place in the list.
enum Admission {
    Connected {
        index: usize,
        confirm_by: i64,
    },
    Pending(usize),
    /// The same, taken out of the list once it's saved.
    Found(Device, i64),
    Waits(Device),
    Turned(v1::ErrorCode, String),
    Unknown,
}

/// What an assertion that doesn't count means to the device showing it.
fn refusal(refused: crate::account::AssertionRefusal) -> (v1::ErrorCode, String) {
    use crate::account::AssertionRefusal as Refused;
    match refused {
        Refused::Expired => (v1::ErrorCode::SignInRequired, "This device's sign-in ran out. Connect again to sign in afresh.".to_owned()),
        Refused::OtherAccount => (v1::ErrorCode::Unauthorized, "This Claktory belongs to another account.".to_owned()),
        Refused::NotSignedIn => (
            v1::ErrorCode::Unauthorized,
            "This Claktory isn't signed in to an account, so it can't be found this way. Add this device with a code from its Settings → Devices.".to_owned(),
        ),
        Refused::Invalid | Refused::OtherKey => {
            (v1::ErrorCode::Unauthorized, "This device's sign-in couldn't be checked. Sign out and in again on it.".to_owned())
        }
    }
}

/// A device's own name, as it gives it: whitespace squeezed, at most
/// `NAME_CHARS`.
fn own_name(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(NAME_CHARS).collect()
}

/// Who may come in, changing `devices` as that says (a key bound, a device
/// added or waiting, a grace started). `shown`: what the device's assertion
/// showed, if it brought one (the account it's signed in as, which
/// `check_assertion` already matched with `owner`'s), or why it doesn't
/// count.
///
/// - A pairing code's token: as before, its device, or the first key to
///   use the code waits to be allowed; with an assertion for the owner's
///   account too, it's in at once (a code and the account: two factors).
/// - A device found through the account: the first device, while this
///   Claktory signed in less than `FIRST_DEVICE_WINDOW` ago and has no
///   device yet (this machine's app aside), is in at once; any other
///   waits to be allowed. Every Hello shows a fresh assertion, first and
///   later ones alike: its key alone is never enough, because anything
///   that borrows the device's tunnel shares its key (another process on
///   the device dialling the app's forwarded port, say).
/// - While this Claktory is signed in, a paired device that isn't
///   confirmed as the owner's is asked to: its grace starts at its first
///   Hello from an app that can confirm, and once it's over the device is
///   turned away until it does. An assertion confirms it.
fn admit(
    devices: &mut Vec<Device>,
    knock: &Knock<'_>,
    peer: &Peer,
    owner: Option<&Owner>,
    shown: Option<Result<String, (v1::ErrorCode, String)>>,
    now: i64,
) -> Admission {
    let signed_in = matches!(&shown, Some(Ok(_)));
    let digest = token_hash(knock.token);
    let mut found = None;
    let mut pending = None;
    for (index, device) in devices.iter_mut().enumerate() {
        // Compared in full every time, whatever matches.
        let known = device.token_sha256.as_bytes();
        let same = known.len() == digest.len() && known.iter().zip(digest.as_bytes()).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0;
        // On the loopback port only this machine's app's token counts; a
        // device found through the account has no code.
        if !same || knock.token.is_empty() || (peer.endpoint_id.is_empty() && !device.local) || device.discovered {
            continue;
        }
        if device.local {
            if peer.loopback || device.endpoint_id == peer.endpoint_id {
                found = Some(index);
            }
        } else if !device.endpoint_id.is_empty() {
            if device.endpoint_id == peer.endpoint_id {
                found = Some(index);
            }
        } else if device.awaiting(now) {
            if device.pending_endpoint_id.is_empty() {
                device.pending_endpoint_id.clone_from(&peer.endpoint_id);
                device.pending_since = Some(now);
                let name = own_name(knock.name);
                if device.unnamed && !name.is_empty() {
                    device.name = name;
                    device.unnamed = false;
                }
            }
            if device.pending_endpoint_id == peer.endpoint_id {
                device.pending_kind = knock.kind.into();
                if let Some(Ok(account)) = &shown {
                    // A code and the owner's account: in without Allow.
                    device.confirmed_as.clone_from(account);
                    device.pending_endpoint_id.clear();
                    device.pending_kind = 0;
                    device.pending_since = None;
                    device.pending_signed_in = false;
                    device.code_expires = None;
                    found = Some(index);
                } else {
                    pending = Some(index);
                }
            }
        }
    }
    // A device found through the account (through the tunnel: the loopback
    // port has none). Its key alone is never enough: it shows a fresh
    // assertion on every Hello, since anything that borrows the device's
    // tunnel shares its key.
    if found.is_none() && pending.is_none() && !peer.endpoint_id.is_empty() {
        let mut known = false;
        for (index, device) in devices.iter_mut().enumerate().filter(|(_, device)| device.discovered && !device.local) {
            if device.endpoint_id == peer.endpoint_id {
                known = true;
                if signed_in {
                    found = Some(index);
                }
            } else if device.endpoint_id.is_empty() && device.awaiting(now) && device.pending_endpoint_id == peer.endpoint_id {
                known = true;
                if signed_in {
                    device.pending_kind = knock.kind.into();
                    pending = Some(index);
                }
            }
        }
        if known && !signed_in {
            return match shown {
                Some(Err((code, reason))) => Admission::Turned(code, reason),
                _ => Admission::Turned(
                    v1::ErrorCode::SignInRequired,
                    "Sign in to your Claktory account on this device to connect.".to_owned(),
                ),
            };
        }
        if found.is_none() && pending.is_none() {
            match &shown {
                Some(Ok(account)) => {
                    let fresh = owner
                        .is_some_and(|owner| owner.signed_in_ms > 0 && now - owner.signed_in_ms < FIRST_DEVICE_WINDOW.as_millis() as i64);
                    let first = fresh && !devices.iter().any(|device| !device.local && !device.endpoint_id.is_empty());
                    let name = own_name(knock.name);
                    let (id, name) = new_id(devices, if name.is_empty() { UNNAMED } else { &name });
                    let mut device = Device {
                        id,
                        name,
                        // For the four digits both sides show (PendingDevice.number).
                        token_sha256: token_hash(knock.assertion),
                        created: now,
                        discovered: true,
                        ..Device::default()
                    };
                    if first {
                        device.endpoint_id.clone_from(&peer.endpoint_id);
                        device.confirmed_as.clone_from(account);
                        found = Some(devices.len());
                    } else {
                        device.code_expires = Some(now + CODE_LIFETIME.as_millis() as i64);
                        device.pending_endpoint_id.clone_from(&peer.endpoint_id);
                        device.pending_kind = knock.kind.into();
                        device.pending_since = Some(now);
                        device.pending_signed_in = true;
                        pending = Some(devices.len());
                    }
                    devices.push(device);
                }
                Some(Err((code, reason))) => return Admission::Turned(*code, reason.clone()),
                None if knock.token.is_empty() && owner.is_some() => {
                    return Admission::Turned(
                        v1::ErrorCode::SignInRequired,
                        "Sign in to your Claktory account on this device to connect.".to_owned(),
                    );
                }
                None => {}
            }
        }
    }
    if let Some(index) = pending {
        return Admission::Pending(index);
    }
    let Some(index) = found else { return Admission::Unknown };
    let device = &mut devices[index];
    // Is it the owner's?
    let mut confirm_by = 0;
    if let Some(owner) = owner
        && !device.local
    {
        if signed_in {
            device.confirmed_as.clone_from(&owner.account_id);
            device.confirm_by = None;
        } else if device.confirmed_as != owner.account_id {
            match device.confirm_by {
                Some(by) if by <= now => {
                    return Admission::Turned(
                        v1::ErrorCode::SignInRequired,
                        "Sign in to your Claktory account on this device to keep using it here.".to_owned(),
                    );
                }
                Some(by) => confirm_by = by,
                None if knock.can_confirm => {
                    confirm_by = now + CONFIRM_GRACE.as_millis() as i64;
                    device.confirm_by = Some(confirm_by);
                }
                None => {}
            }
        }
    }
    if !peer.endpoint_id.is_empty() {
        device.endpoint_id.clone_from(&peer.endpoint_id);
    }
    device.last_seen = Some(now);
    device.last_address = peer.address.map(|address| address.ip().to_canonical().to_string()).unwrap_or_default();
    Admission::Connected { index, confirm_by }
}

/// Where a device waiting to be allowed stands.
pub(crate) enum Pairing {
    Waiting,
    Allowed,
    /// Turned away, cancelled or expired: what to tell it.
    Ended(String),
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
    /// When it has to confirm it's the owner's by (Welcome.confirm_by_ms);
    /// 0 when it needn't.
    pub confirm_by: i64,
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
    /// Who may come in with `knock`, and how: the paired device whose token
    /// it brings, if `peer` may use it (the key bound to it; for this
    /// machine's app, any key from this machine), or one found through the
    /// person's account (see `admit`). Connected until the returned
    /// `Connected` goes. A key turned away is answered no sooner than a
    /// second after its last.
    pub(crate) async fn authorize(self: &Arc<Self>, knock: &Knock<'_>, peer: &Peer) -> Authorized {
        let now = now_ms();
        let account = self.lock().config.account.clone();
        let owner =
            crate::account::owner(self, &account).map(|owner| Owner { account_id: owner.account_id, signed_in_ms: owner.signed_in_ms });
        // What its assertion shows, if it brought one through the tunnel.
        let shown = (!knock.assertion.is_empty() && !peer.endpoint_id.is_empty()).then(|| {
            crate::account::check_assertion(self, &account, knock.assertion, &peer.endpoint_id)
                .map(|assertion| assertion.account_id)
                .map_err(refusal)
        });
        let mut admission = Admission::Unknown;
        let changing = devices_lock();
        match devices(&self.home) {
            Ok(mut devices) => {
                admission = admit(&mut devices, knock, peer, owner.as_ref(), shown, now);
                if matches!(admission, Admission::Connected { .. } | Admission::Pending(_))
                    && let Err(error) = save_devices(&self.home, &devices)
                {
                    tracing::warn!(%error, "could not note when a device connected");
                    admission = Admission::Unknown;
                }
                admission = match admission {
                    Admission::Connected { index, confirm_by } => Admission::Found(devices.swap_remove(index), confirm_by),
                    Admission::Pending(index) => Admission::Waits(devices.swap_remove(index)),
                    other => other,
                };
            }
            Err(error) => tracing::error!(%error, "could not read the paired devices"),
        }
        drop(changing);
        let (device, confirm_by) = match admission {
            Admission::Found(device, confirm_by) => (device, confirm_by),
            Admission::Waits(device) => {
                tracing::info!(device = %device.id, endpoint = %peer.endpoint_id, discovered = device.discovered, "a device waits to be allowed");
                // An earlier code's device by the same id ended some way: not this one.
                lock(&self.remote.ended).remove(&device.id);
                self.refresh_remote();
                return Authorized::Pending(device);
            }
            Admission::Turned(code, reason) => {
                tracing::warn!(endpoint = %peer.endpoint_id, ?code, "turned a device away");
                self.pause_after_refusal(peer).await;
                return Authorized::Turned(code, reason);
            }
            Admission::Unknown | Admission::Connected { .. } | Admission::Pending(_) => {
                // The loopback port's refusals are logged where they're known.
                if !peer.endpoint_id.is_empty() {
                    tracing::warn!(endpoint = %peer.endpoint_id, "refused a device that isn't paired");
                }
                self.pause_after_refusal(peer).await;
                return Authorized::Refused;
            }
        };
        let closed = Arc::new(tokio::sync::Notify::new());
        let session = Session { closed: closed.clone(), endpoint_id: peer.endpoint_id.clone() };
        lock(&self.remote.connected).entry(device.id.clone()).or_default().push(session);
        self.refresh_remote();
        Authorized::Connected(Connected { device, confirm_by, closed, factory: Arc::downgrade(self) })
    }

    /// ConfirmDevice, from `asker` (its device id; `LOCAL_DEVICE` for this
    /// machine's app), connected with key `endpoint_id`: with an assertion,
    /// the asker confirms itself; with a device id, the person confirms that
    /// device from this one, which must be confirmed itself.
    pub async fn confirm_device(&self, asker: &str, endpoint_id: &str, request: &v1::ConfirmDevice) -> Result<(), RequestError> {
        let account = self.lock().config.account.clone();
        let Some(owner) = crate::account::owner(self, &account) else {
            return Err(RequestError::Account(
                v1::ErrorCode::InvalidRequest,
                "This Claktory isn't signed in to an account, so there's nothing to confirm.".to_owned(),
            ));
        };
        let target = if request.assertion.is_empty() {
            let trusted = asker == LOCAL_DEVICE
                || devices(&self.home)
                    .map_err(|error| RequestError::Remote(error.to_string()))?
                    .iter()
                    .any(|device| device.id == asker && device.confirmed_as == owner.account_id);
            if !trusted {
                return Err(RequestError::Account(
                    v1::ErrorCode::Unauthorized,
                    "Confirm this device first: sign in to your account on it. Then you can confirm others from it.".to_owned(),
                ));
            }
            request.device_id.clone()
        } else {
            // This computer's app is the owner's already.
            if asker == LOCAL_DEVICE {
                return Ok(());
            }
            crate::account::check_assertion(self, &account, &request.assertion, endpoint_id).map_err(|refused| {
                let (code, reason) = refusal(refused);
                RequestError::Account(code, reason)
            })?;
            asker.to_owned()
        };
        {
            let _changing = devices_lock();
            let mut devices = devices(&self.home).map_err(|error| RequestError::Remote(error.to_string()))?;
            let Some(device) = devices.iter_mut().find(|device| device.id == target && !device.local && !device.endpoint_id.is_empty())
            else {
                return Err(RequestError::UnknownDevice(target));
            };
            device.confirmed_as.clone_from(&owner.account_id);
            device.confirm_by = None;
            save_devices(&self.home, &devices).map_err(|error| RequestError::Remote(error.to_string()))?;
        }
        tracing::info!(device = %target, by = %asker, "a device was confirmed as the owner's");
        self.refresh_remote();
        Ok(())
    }

    /// Waits until a second after `peer`'s last refusal.
    async fn pause_after_refusal(&self, peer: &Peer) {
        let wait = {
            let now = Instant::now();
            let last = lock(&self.remote.failures).insert(peer.endpoint_id.clone(), now);
            last.map(|last| FAILURE_PAUSE.saturating_sub(now.duration_since(last))).unwrap_or_default()
        };
        tokio::time::sleep(wait).await;
    }

    /// Where the device `id`, waiting with key `endpoint_id`, stands.
    pub(crate) fn pairing(&self, waiting: &Device, endpoint_id: &str) -> Pairing {
        let (id, now) = (waiting.id.as_str(), now_ms());
        let device = devices(&self.home).ok().and_then(|devices| devices.into_iter().find(|device| device.id == id));
        let ended = match &device {
            Some(device) if device.endpoint_id == endpoint_id => return Pairing::Allowed,
            Some(device) if device.is_pending(now) && device.pending_endpoint_id == endpoint_id => return Pairing::Waiting,
            Some(device) if device.expired(now) => Some(Ended::Expired),
            _ => lock(&self.remote.ended).get(id).copied(),
        };
        let machine = machine_name();
        // One that came through the account asks again by connecting again.
        if waiting.discovered {
            return Pairing::Ended(match ended {
                Some(Ended::Expired) => format!("No one allowed this device on {machine} in time. Connect again to ask again."),
                Some(Ended::Cancelled) => format!("Connecting this device was cancelled on {machine}. Connect again to ask again."),
                Some(Ended::Denied) | None => format!("{machine} didn't allow this device."),
            });
        }
        Pairing::Ended(match ended {
            Some(Ended::Expired) => {
                format!("The code expired before this device was allowed. Add it again on {machine} in Settings → Devices.")
            }
            Some(Ended::Cancelled) => {
                format!("Adding this device was cancelled on {machine}. To try again, add it again there in Settings → Devices.")
            }
            Some(Ended::Denied) | None => {
                format!("{machine} didn't allow this device. To try again, add it again on {machine} in Settings → Devices.")
            }
        })
    }

    /// Keeps remote access on, while it lives, for a device's connection
    /// that waits to be allowed, so it hears how that ended.
    pub(crate) fn waiting(self: &Arc<Self>) -> Waiting {
        self.remote.waiting.fetch_add(1, Ordering::Relaxed);
        Waiting { factory: Arc::downgrade(self) }
    }

    /// Waits until a waiting device is allowed or its code ends, or a second.
    pub(crate) async fn pairing_changed(&self) {
        let _ = tokio::time::timeout(CHECK, self.remote.pairing.notified()).await;
    }

    /// A code that let no device in ended: the device waiting with it hears
    /// why at once.
    fn pairing_ended(&self, device: &Device, ended: Ended) {
        tracing::info!(device = %device.id, ?ended, "a pairing code ended");
        lock(&self.remote.ended).insert(device.id.clone(), ended);
        self.remote.pairing.notify_waiters();
    }

    /// Allows the device waiting with this device's code, from the app.
    pub async fn approve_device(&self, id: &str) -> Result<(), RequestError> {
        match approve(&self.home, id) {
            Ok(device) => tracing::info!(device = %device.id, "the person allowed a device"),
            Err(RemoteError::NotWaiting(id)) => return Err(RequestError::UnknownDevice(id)),
            Err(error) => return Err(RequestError::Remote(error.to_string())),
        }
        self.remote.pairing.notify_waiters();
        self.refresh_remote();
        Ok(())
    }

    /// Turns away the device waiting with this device's code, from the app,
    /// and forgets it.
    pub async fn deny_device(&self, id: &str) -> Result<(), RequestError> {
        match deny(&self.home, id) {
            Ok(device) => self.pairing_ended(&device, Ended::Denied),
            Err(RemoteError::NotWaiting(id)) => return Err(RequestError::UnknownDevice(id)),
            Err(error) => return Err(RequestError::Remote(error.to_string())),
        }
        self.refresh_remote();
        Ok(())
    }

    /// Ends a code that hasn't let a device in, from the app (Add a device
    /// closed). Nothing to do for one that's gone or let a device in.
    pub async fn cancel_pairing(&self, id: &str) -> Result<(), RequestError> {
        match cancel(&self.home, id) {
            Ok(device) => self.pairing_ended(&device, Ended::Cancelled),
            Err(RemoteError::NotWaiting(_)) => return Ok(()),
            Err(error) => return Err(RequestError::Remote(error.to_string())),
        }
        self.refresh_remote();
        Ok(())
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

    /// Whether this Claktory is signed in to an account, so the person's
    /// devices may find it; the gate follows.
    pub(crate) fn account_open(&self) -> bool {
        let account = self.lock().config.account.clone();
        let open = crate::account::owner(self, &account).is_some();
        self.remote.account_open.store(open, Ordering::Relaxed);
        open
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
        let on = remote_on(&devices, now) || self.account_open();
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
        self.remote.changed.notify_one();
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
    pub(crate) fn remote_view(&self, inner: &Inner) -> v1::RemoteAccess {
        let owner = crate::account::owner(self, &inner.config.account);
        let shown = lock(&self.remote.shown).clone();
        let connected = lock(&self.remote.connected);
        let now = now_ms();
        let mut pending: Vec<v1::PendingDevice> = shown
            .devices
            .iter()
            .filter(|device| device.is_pending(now))
            .map(|device| v1::PendingDevice {
                device_id: device.id.clone(),
                name: device.name.clone(),
                kind: device.pending_kind,
                number: approval_number(&shown.endpoint_id, &device.pending_endpoint_id, &device.token_sha256),
                requested_ms: device.pending_since.unwrap_or_default(),
                expires_ms: device.code_expires.unwrap_or_default(),
                signed_in: device.pending_signed_in,
                discovered: device.discovered,
            })
            .collect();
        pending.sort_by_key(|device| device.requested_ms);
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
                    // Whether it's the owner's: only asked while signed in.
                    let confirmed = owner.as_ref().is_some_and(|owner| device.confirmed_as == owner.account_id);
                    let confirm_by_ms = match &owner {
                        Some(_) if !confirmed => device.confirm_by.unwrap_or_default(),
                        _ => 0,
                    };
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
                        confirmed,
                        confirm_by_ms,
                    }
                })
                .collect(),
            pending_devices: pending,
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
        let code = self.code_with(&token).await?;
        Ok(v1::PairingCode { code, device_id: device.id })
    }

    /// The pairing code with `token`: remote access turns on, and the code
    /// names the relay the tunnel reaches and the addresses it can be
    /// dialled at.
    async fn code_with(self: &Arc<Self>, token: &str) -> Result<String, RequestError> {
        let (endpoint, relays, addresses) = self.reachable().await?;
        Ok(pairing_code(token, &endpoint.id().to_string(), &relays, &addresses, &self.identity()))
    }

    /// The endpoint other machines reach, once remote access is on (as what
    /// asks for it wants), and where they reach it: its relays and its
    /// addresses. Waits a moment for a relay.
    pub(crate) async fn reachable(self: &Arc<Self>) -> Result<(Endpoint, Vec<String>, Vec<SocketAddr>), RequestError> {
        sync_tunnel(self).await;
        self.refresh_remote();
        let Some(endpoint) = lock(&self.remote.endpoint).clone() else {
            let error = lock(&self.remote.shown).error.clone();
            return Err(RequestError::Remote(if error.is_empty() { "The private tunnel isn't running.".to_owned() } else { error }));
        };
        let wait = lock(&self.remote.online_wait).unwrap_or(ONLINE_WAIT);
        let _ = tokio::time::timeout(wait, endpoint.online()).await;
        let (relays, addresses) = self.reached_at(&endpoint);
        Ok((endpoint, relays, addresses))
    }

    /// Where other machines reach `endpoint` now: its relays (config.json's
    /// too) and its addresses but 127.0.0.1's.
    pub(crate) fn reached_at(&self, endpoint: &Endpoint) -> (Vec<String>, Vec<SocketAddr>) {
        let addr = endpoint.addr();
        let mut relays: Vec<String> = addr.relay_urls().map(ToString::to_string).collect();
        for relay in self.lock().config.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default() {
            if !relays.iter().any(|known| known.trim_end_matches('/') == relay.trim_end_matches('/')) {
                relays.push(relay);
            }
        }
        let addresses: Vec<SocketAddr> = addr.ip_addrs().filter(|address| !address.ip().is_loopback()).copied().collect();
        (relays, addresses)
    }

    /// The endpoint other machines reach, while remote access is on.
    pub(crate) fn remote_endpoint(&self) -> Option<Endpoint> {
        lock(&self.remote.endpoint).clone()
    }

    /// Keeps the token of an agent's pairing in memory while its code waits.
    pub(crate) fn keep_code(&self, pairing: &str, token: String) {
        lock(&self.remote.codes).insert(pairing.to_owned(), token);
    }

    /// Forgets the token of an agent's pairing that no longer waits.
    pub(crate) fn forget_code(&self, pairing: &str) {
        lock(&self.remote.codes).remove(pairing);
    }

    /// The code of an agent's pairing whose device (`id`, made at
    /// `created`) waits for it: the one made with it, or after a restart a
    /// new one. Err(NoPairingCode) once it no longer waits.
    pub(crate) async fn agent_pairing_code(self: &Arc<Self>, pairing: &str, id: &str, created: i64) -> Result<String, RequestError> {
        let gone = || RequestError::NoPairingCode("This code no longer works: the device was added, or adding it ended.".to_owned());
        let kept = lock(&self.remote.codes).get(pairing).cloned();
        let token = match kept {
            Some(token) => token,
            None => {
                let (home, id) = (self.home.clone(), id.to_owned());
                let token = match tokio::task::spawn_blocking(move || rekey(&home, &id, created)).await.map_err(std::io::Error::other)? {
                    Ok(token) => token,
                    Err(RemoteError::NotWaiting(_)) => return Err(gone()),
                    Err(error) => return Err(RequestError::Remote(error.to_string())),
                };
                lock(&self.remote.codes).entry(pairing.to_owned()).or_insert(token).clone()
            }
        };
        self.code_with(&token).await
    }

    /// Where the device `id`, made at `created` with a code good until
    /// `expires`, stands, from `devices` (devices.json as it is now).
    pub(crate) fn pairing_state(&self, devices: &[Device], id: &str, created: i64, expires: i64) -> v1::DevicePairingState {
        use v1::DevicePairingState as State;
        let now = now_ms();
        match devices.iter().find(|device| device.id == id && device.created == created && !device.local) {
            Some(device) if !device.endpoint_id.is_empty() => State::Added,
            Some(device) if device.is_pending(now) => State::Asking,
            Some(device) if device.awaiting(now) => State::Waiting,
            Some(_) => State::Expired,
            None => match lock(&self.remote.ended).get(id) {
                Some(Ended::Denied) => State::Denied,
                Some(Ended::Expired) => State::Expired,
                Some(Ended::Cancelled) => State::Cancelled,
                None if expires <= now => State::Expired,
                None => State::Cancelled,
            },
        }
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
        self.cancel_device_flows(&removed.id);
        self.refresh_remote();
        // Its zoom goes with it.
        if self.lock().config.display.zoom.contains_key(&removed.id)
            && let Err(error) = self.set_zoom(&removed.id, 1.0).await
        {
            tracing::warn!(%error, device = %removed.id, "could not forget an unpaired device's zoom");
        }
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

/// Keeps the tunnel as it should be: on 127.0.0.1 always, and also on
/// every address with relays while a device on another machine can connect,
/// within a second of a change. Also keeps what the Workspace shows of it
/// current, and forgets codes that ran out.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    if let Some(strong) = factory.upgrade() {
        crate::pipe_link::reload(&strong);
    }
    while let Some(strong) = factory.upgrade() {
        match forget_expired(&strong.home) {
            Ok(expired) => {
                for device in &expired {
                    strong.pairing_ended(device, Ended::Expired);
                }
            }
            Err(error) => tracing::error!(%error, "could not forget expired pairing codes"),
        }
        sync_tunnel(&strong).await;
        strong.refresh_remote();
        drop(strong);
        tokio::time::sleep(CHECK).await;
    }
}

/// Starts the tunnel's endpoints, or changes them when they should change:
/// the loopback one once, the remote one as remote access and its relays
/// say. Boxed: a client the tunnel serves can pair a device, which comes
/// back here.
fn sync_tunnel(factory: &Arc<Factory>) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
    Box::pin(async move {
        let devices = devices(&factory.home).unwrap_or_else(|error| {
            tracing::error!(%error, "could not read the paired devices");
            Vec::new()
        });
        let relays = factory.lock().config.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default();
        let waiting = factory.remote.waiting.load(Ordering::Relaxed) > 0;
        let pipes = factory.remote.pipes.wants_tunnel(now_ms());
        let open = factory.account_open();
        let wanted = Wanted { on: remote_on(&devices, now_ms()) || waiting || pipes || open, relays };
        let mut tunnel = factory.remote.tunnel.lock().await;
        let remote_as_wanted = match &tunnel.remote {
            Some((relays, ..)) => wanted.on && *relays == wanted.relays,
            None => !wanted.on,
        };
        if (tunnel.local.is_some() && remote_as_wanted) || tunnel.failed.as_ref() == Some(&wanted) {
            return;
        }
        let mut failed = None;
        if tunnel.local.is_none() {
            match start_local(factory).await {
                Ok(endpoint) => {
                    let accept = tokio::spawn(accept(endpoint.clone(), Arc::downgrade(factory)));
                    let id = endpoint.id().to_string();
                    tracing::info!(endpoint = %id, "private tunnel started on 127.0.0.1");
                    tunnel.local = Some((endpoint, accept));
                    factory.set_remote_shown(|shown| shown.endpoint_id = id);
                }
                Err(error) => failed = Some(error),
            }
        }
        if failed.is_none() && !remote_as_wanted {
            if let Some((_, endpoint, accept)) = tunnel.remote.take() {
                lock(&factory.remote.endpoint).take();
                accept.abort();
                endpoint.close().await;
                tracing::info!("remote access stopped");
            }
            if wanted.on {
                match start_remote(&factory.home, &wanted, &factory.remote.pipes.admits, &factory.remote.account_open).await {
                    Ok(endpoint) => {
                        tracing::info!(endpoint = %endpoint.id(), "remote access started");
                        let accept = tokio::spawn(accept(endpoint.clone(), Arc::downgrade(factory)));
                        *lock(&factory.remote.endpoint) = Some(endpoint.clone());
                        tunnel.remote = Some((wanted.relays.clone(), endpoint, accept));
                    }
                    Err(error) => failed = Some(error),
                }
            }
        }
        match failed {
            None => {
                tunnel.failed = None;
                factory.set_remote_shown(|shown| shown.error.clear());
            }
            Some(error) => {
                tracing::error!(%error, "could not start the private tunnel");
                tunnel.failed = Some(wanted);
                factory.set_remote_shown(|shown| shown.error = error.to_string());
            }
        }
    })
}

/// The loopback endpoint, on the port this machine's pairing code names
/// when it's free, and the code written for it.
async fn start_local(factory: &Arc<Factory>) -> Result<Endpoint, RemoteError> {
    let home = factory.home.clone();
    let key = {
        let home = home.clone();
        tokio::task::spawn_blocking(move || secret_key(&home)).await.map_err(std::io::Error::other)??
    };
    let pipes = factory.remote.pipes.admits.clone();
    // Local connections are this machine's whoever is signed in.
    let closed = Arc::new(AtomicBool::new(false));
    let endpoint = bind(&home, key, &Wanted { on: false, relays: Vec::new() }, local_port(&home), &pipes, &closed).await?;
    let (id, port) = (endpoint.id().to_string(), loopback_port(&endpoint).unwrap_or_default());
    let written = tokio::task::spawn_blocking(move || write_local_pairing(&home, &id, port)).await.map_err(std::io::Error::other)?;
    if let Err(error) = written {
        endpoint.close().await;
        return Err(error);
    }
    Ok(endpoint)
}

/// The endpoint for devices on other machines, on the port it had before
/// when it's free, so the addresses in their codes stay right.
async fn start_remote(
    home: &Home,
    wanted: &Wanted,
    pipes: &Arc<crate::pipe_link::Admits>,
    open: &Arc<AtomicBool>,
) -> Result<Endpoint, RemoteError> {
    let key = {
        let home = home.clone();
        tokio::task::spawn_blocking(move || secret_key(&home)).await.map_err(std::io::Error::other)??
    };
    let path = dir(home).join("remote-port");
    let port = std::fs::read_to_string(&path).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0);
    let endpoint = bind(home, key, wanted, port, pipes, open).await?;
    if let Some(bound) = loopback_port(&endpoint)
        && bound != port
        && let Err(error) = write_private(&path, format!("{bound}\n").as_bytes())
    {
        tracing::warn!(%error, "could not save the remote access port");
    }
    Ok(endpoint)
}

/// The port the endpoint answers on over IPv4 (at 127.0.0.1 for the
/// loopback one).
fn loopback_port(endpoint: &Endpoint) -> Option<u16> {
    endpoint.bound_sockets().into_iter().find(SocketAddr::is_ipv4).map(|address| address.port())
}

/// An endpoint as `wanted` says, on `port` (the local pairing code's) if it's
/// free. Without remote access: 127.0.0.1 only, no relays, nothing published.
/// With it: every address, the relays (n0's, or config.json's own), and
/// n0's address lookup either way: where this Claktory can be reached (its
/// key, home relay and addresses, never data) stays published, so every
/// already-paired device follows a relay change without re-pairing.
/// Never a port mapping (no portmapper).
async fn bind(
    home: &Home,
    key: SecretKey,
    wanted: &Wanted,
    port: u16,
    pipes: &Arc<crate::pipe_link::Admits>,
    open: &Arc<AtomicBool>,
) -> Result<Endpoint, RemoteError> {
    let failed = |error: &dyn std::fmt::Display| RemoteError::Tunnel(error.to_string());
    let builder = |port: u16| -> Result<Builder, RemoteError> {
        let builder = if wanted.on { Endpoint::builder(presets::N0) } else { Endpoint::builder(presets::Minimal) };
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
            .alpns(vec![ALPN_COMPRESSED.to_vec(), ALPN.to_vec(), ALPN_FORWARD.to_vec(), crate::pipe_link::ALPN_PIPE.to_vec()])
            .hooks(Gate { home: home.clone(), pipes: pipes.clone(), open: open.clone() })
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
    if connection.alpn() == crate::pipe_link::ALPN_PIPE {
        crate::pipe_link::serve(connection, factory.clone()).await;
        forget_link(&factory, &endpoint_id, number);
        return;
    }
    if connection.alpn() == ALPN_FORWARD {
        crate::forward::serve(connection, factory.clone(), endpoint_id.clone()).await;
        forget_link(&factory, &endpoint_id, number);
        return;
    }
    let compress = connection.alpn() == ALPN_COMPRESSED;
    tracing::info!(endpoint = %endpoint_id, compressed = compress, "tunnel connection opened");
    // A connection that drops (the device slept, or its network changed)
    // ends all its streams at once: said once, not once each.
    let dropped = Arc::new(AtomicBool::new(false));
    while let Ok((send, receive)) = connection.accept_bi().await {
        let Some(strong) = factory.upgrade() else { break };
        let address = path_of(&connection).1;
        let peer = Peer { endpoint_id: endpoint_id.clone(), loopback: from_this_machine(address), address };
        let (link, dropped) = (connection.clone(), dropped.clone());
        tokio::spawn(async move {
            let endpoint = peer.endpoint_id.clone();
            let served = if compress {
                crate::connection::serve_stream(compressed(receive, send), strong, Some(peer)).await
            } else {
                crate::connection::serve_stream(tokio::io::join(receive, send), strong, Some(peer)).await
            };
            match served {
                Ok(()) => {}
                Err(error) if link.close_reason().is_some() || error.to_string().contains("connection lost") => {
                    if !dropped.swap(true, Ordering::Relaxed) {
                        tracing::info!(%endpoint, %error, "a device's tunnel connection dropped");
                    }
                }
                Err(error) => tracing::warn!(%endpoint, %error, "tunnel connection failed"),
            }
        });
    }
    forget_link(&factory, &endpoint_id, number);
}

/// A tunnel connection that ended no longer needs closing on unpair.
fn forget_link(factory: &Weak<Factory>, endpoint_id: &str, number: u64) {
    if let Some(strong) = factory.upgrade() {
        let mut links = lock(&strong.remote.links);
        if let Some(open) = links.get_mut(endpoint_id) {
            open.retain(|(known, _)| *known != number);
            if open.is_empty() {
                links.remove(endpoint_id);
            }
        }
    }
}

/// The id of the paired device whose key `endpoint_id` is, once it was
/// allowed; None for any other key, a device still waiting included.
pub(crate) fn paired_device(home: &Home, endpoint_id: &str) -> Option<String> {
    let devices = devices(home).ok()?;
    devices.into_iter().find(|device| !device.endpoint_id.is_empty() && device.endpoint_id == endpoint_id).map(|device| device.id)
}

/// A stream on `claktory/2`: the bytes `claktory/1` carries, each way one
/// zstd stream, flushed whenever what's written is (the WebSocket flushes
/// after every message), so each message arrives whole at once.
pub fn compressed<R, W>(read: R, write: W) -> tokio::io::Join<impl tokio::io::AsyncRead, impl tokio::io::AsyncWrite>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    use async_compression::tokio::{bufread::ZstdDecoder, write::ZstdEncoder};
    tokio::io::join(ZstdDecoder::new(tokio::io::BufReader::new(read)), ZstdEncoder::with_quality(write, COMPRESSION))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandbox_chooses_the_machine_name_shown() {
        let chosen = |value: &str| chosen_machine_name(Some(value.into()));
        assert_eq!(chosen("  qa-video ").as_deref(), Some("qa-video"));
        assert_eq!(chosen("Studio").as_deref(), Some("Studio"));
        assert_eq!(chosen_machine_name(None), None, "unset: the machine's own");
        assert_eq!(chosen("   "), None);
        assert_eq!(chosen("Mac\nmini"), None, "no control characters");
        assert_eq!(chosen(&"x".repeat(100)).map(|name| name.chars().count()), Some(64));
    }

    #[test]
    fn values_in_pairing_codes_are_percent_encoded() {
        assert_eq!(encode("Mac mini/2"), "Mac%20mini%2F2");
        assert_eq!(encode("box-1.lan"), "box-1.lan");
        assert_eq!(decode("Mac%20mini%2F2"), "Mac mini/2");
        assert_eq!(decode("100%"), "100%");
        let claktory = Identity { name: "Lee's Mac".into(), color: "teal".into() };
        let code = pairing_code("ab", "id", &["https://relay.example/".into()], &["127.0.0.1:7".parse().unwrap()], &claktory);
        assert_eq!(code_values(&code, "name").collect::<Vec<_>>(), ["Lee's Mac"]);
        assert_eq!(code_values(&code, "color").collect::<Vec<_>>(), ["teal"]);
        assert_eq!(code_values(&code, "r").collect::<Vec<_>>(), ["https://relay.example/"]);
        assert_eq!(code_values(&code, "a").collect::<Vec<_>>(), ["127.0.0.1:7"]);
        assert_eq!(code_values(&code, "t").collect::<Vec<_>>(), ["ab"]);
    }

    #[test]
    fn a_claktorys_own_color_comes_from_its_key_and_stays() {
        let root = std::env::temp_dir().join(format!("factoryd-key-color-{}", std::process::id()));
        let homes: Vec<Home> = (0..12).map(|n| Home::new(root.join(n.to_string()))).collect();
        let colors: Vec<&str> = homes.iter().map(key_color).collect();
        assert!(colors.iter().all(|color| crate::config::AGENT_COLORS.contains(color)));
        assert_eq!(homes.iter().map(key_color).collect::<Vec<_>>(), colors, "the same key, the same color");
        assert!(colors.iter().any(|color| *color != colors[0]), "different keys, different colors: {colors:?}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_waiting_code_gets_a_new_token_only_while_no_device_has_used_it() {
        let root = std::env::temp_dir().join(format!("factoryd-rekey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(root.clone());
        let (old, device) = pair(&home, "Phone").expect("paired");
        let new = rekey(&home, &device.id, device.created).expect("a new token");
        assert_ne!(new, old);
        let saved = devices(&home).expect("devices");
        assert_eq!(saved[0].token_sha256, token_hash(&new), "the old code stops working");
        assert!(matches!(rekey(&home, &device.id, device.created + 1), Err(RemoteError::NotWaiting(_))), "another device by that id");
        let mut used = saved;
        used[0].pending_endpoint_id = "a key".into();
        save_devices(&home, &used).expect("save");
        assert!(matches!(rekey(&home, &device.id, device.created), Err(RemoteError::NotWaiting(_))), "a device used it");
        let _ = std::fs::remove_dir_all(root);
    }

    fn knock<'a>(token: &'a str, assertion: &'a str, can_confirm: bool) -> Knock<'a> {
        Knock { token, assertion, can_confirm, kind: v1::DeviceKind::Phone, name: "Matt's  iPhone" }
    }

    fn tunnel(key: &str) -> Peer {
        Peer { endpoint_id: key.to_owned(), loopback: false, address: None }
    }

    fn owner(signed_in_ms: i64) -> Owner {
        Owner { account_id: "acct".to_owned(), signed_in_ms }
    }

    fn ours() -> Option<Result<String, (v1::ErrorCode, String)>> {
        Some(Ok("acct".to_owned()))
    }

    const MINUTE: i64 = 60_000;

    #[test]
    fn the_first_device_through_the_account_is_in_at_once_only_while_the_sign_in_is_fresh() {
        let now = 100 * MINUTE;
        // Fresh: in at once, named after itself, connecting by its key from then on.
        let mut devices = Vec::new();
        let Admission::Connected { index, confirm_by } =
            admit(&mut devices, &knock("", "A1", true), &tunnel("k1"), Some(&owner(now - 29 * MINUTE)), ours(), now)
        else {
            panic!("in at once")
        };
        let first = &devices[index];
        assert_eq!(
            (first.name.as_str(), first.endpoint_id.as_str(), first.confirmed_as.as_str(), confirm_by),
            ("Matt's iPhone", "k1", "acct", 0)
        );
        assert!(first.discovered && first.code_expires.is_none());
        let again = admit(&mut devices, &knock("", "A1", true), &tunnel("k1"), Some(&owner(now - 29 * MINUTE)), ours(), now + 1);
        assert!(matches!(again, Admission::Connected { index: 0, .. }), "back with a fresh assertion");
        // Its key alone is never enough: anything that borrows the device's
        // tunnel shares its key.
        let alone = admit(&mut devices, &knock("", "", true), &tunnel("k1"), Some(&owner(now - 29 * MINUTE)), None, now + 1);
        assert!(matches!(alone, Admission::Turned(v1::ErrorCode::SignInRequired, _)), "not by its key alone");
        let expired = Some(Err(refusal(crate::account::AssertionRefusal::Expired)));
        let stale = admit(&mut devices, &knock("", "A1", true), &tunnel("k1"), Some(&owner(now - 29 * MINUTE)), expired, now + 1);
        assert!(
            matches!(stale, Admission::Turned(v1::ErrorCode::SignInRequired, reason) if reason.contains("ran out")),
            "an assertion that ran out asks for a fresh sign-in"
        );

        // Not the first: it waits to be allowed, its digits from the assertion.
        let Admission::Pending(index) =
            admit(&mut devices, &knock("", "A2", true), &tunnel("k2"), Some(&owner(now - 29 * MINUTE)), ours(), now)
        else {
            panic!("waits")
        };
        let second = devices[index].clone();
        assert!(second.discovered && second.pending_signed_in && second.endpoint_id.is_empty());
        assert_eq!((second.pending_endpoint_id.as_str(), second.token_sha256.clone()), ("k2", token_hash("A2")));
        assert!(
            matches!(admit(&mut devices, &knock("", "A2", true), &tunnel("k2"), None, ours(), now), Admission::Pending(i) if i == index),
            "still waiting"
        );
        assert!(
            matches!(
                admit(&mut devices, &knock("", "", true), &tunnel("k2"), None, None, now),
                Admission::Turned(v1::ErrorCode::SignInRequired, _)
            ),
            "waiting, but never by its key alone"
        );
        // Its assertion is no token: nobody gets in with it.
        assert!(matches!(admit(&mut devices, &knock("A2", "", true), &tunnel("k3"), None, None, now), Admission::Unknown));

        // An old sign-in: even the first device waits.
        let mut devices = Vec::new();
        assert!(matches!(
            admit(&mut devices, &knock("", "A", true), &tunnel("k1"), Some(&owner(now - 31 * MINUTE)), ours(), now),
            Admission::Pending(0)
        ));
        let mut devices = Vec::new();
        assert!(
            matches!(admit(&mut devices, &knock("", "A", true), &tunnel("k1"), Some(&owner(0)), ours(), now), Admission::Pending(0)),
            "signed in before it was kept"
        );
    }

    #[test]
    fn a_device_without_a_good_assertion_is_told_what_to_do() {
        let mut devices = Vec::new();
        let now = 1_000;
        let turned = admit(&mut devices, &knock("", "", true), &tunnel("k"), Some(&owner(now)), None, now);
        assert!(matches!(turned, Admission::Turned(v1::ErrorCode::SignInRequired, _)));
        let refused = Some(Err(refusal(crate::account::AssertionRefusal::OtherAccount)));
        assert!(
            matches!(admit(&mut devices, &knock("", "A", true), &tunnel("k"), Some(&owner(now)), refused, now), Admission::Turned(v1::ErrorCode::Unauthorized, reason) if reason.contains("another account"))
        );
        assert!(
            matches!(admit(&mut devices, &knock("", "", true), &tunnel("k"), None, None, now), Admission::Unknown),
            "not signed in: as before"
        );
        assert!(devices.is_empty());
    }

    #[test]
    fn a_code_and_the_account_need_no_allow_and_paired_devices_confirm_once() {
        let now = 1_000_000;
        let mut devices = vec![Device {
            id: "phone".into(),
            name: "Phone".into(),
            token_sha256: token_hash("T"),
            code_expires: Some(now + MINUTE),
            ..Device::default()
        }];
        let Admission::Connected { confirm_by, .. } =
            admit(&mut devices, &knock("T", "A", true), &tunnel("k"), Some(&owner(1)), ours(), now)
        else {
            panic!("in without Allow")
        };
        assert_eq!(
            (devices[0].endpoint_id.as_str(), devices[0].confirmed_as.as_str(), devices[0].code_expires, confirm_by),
            ("k", "acct", None, 0)
        );

        // A device from before: an old app keeps working as it did…
        let mut devices = vec![Device { id: "mac".into(), token_sha256: token_hash("M"), endpoint_id: "m".into(), ..Device::default() }];
        assert!(matches!(
            admit(&mut devices, &knock("M", "", false), &tunnel("m"), Some(&owner(1)), None, now),
            Admission::Connected { confirm_by: 0, .. }
        ));
        assert_eq!(devices[0].confirm_by, None);
        // …and one that can confirm gets a week, once…
        let grace = now + CONFIRM_GRACE.as_millis() as i64;
        assert!(
            matches!(admit(&mut devices, &knock("M", "", true), &tunnel("m"), Some(&owner(1)), None, now), Admission::Connected { confirm_by, .. } if confirm_by == grace)
        );
        assert!(
            matches!(admit(&mut devices, &knock("M", "", true), &tunnel("m"), Some(&owner(1)), None, now + MINUTE), Admission::Connected { confirm_by, .. } if confirm_by == grace)
        );
        // …then has to.
        let after = admit(&mut devices, &knock("M", "", false), &tunnel("m"), Some(&owner(1)), None, grace + 1);
        assert!(matches!(after, Admission::Turned(v1::ErrorCode::SignInRequired, _)));
        assert!(matches!(
            admit(&mut devices, &knock("M", "A", true), &tunnel("m"), Some(&owner(1)), ours(), grace + 1),
            Admission::Connected { confirm_by: 0, .. }
        ));
        assert_eq!((devices[0].confirmed_as.as_str(), devices[0].confirm_by), ("acct", None));
        // Not signed in, nothing is asked.
        let mut devices = vec![Device { id: "mac".into(), token_sha256: token_hash("M"), endpoint_id: "m".into(), ..Device::default() }];
        assert!(matches!(
            admit(&mut devices, &knock("M", "", true), &tunnel("m"), None, None, now),
            Admission::Connected { confirm_by: 0, .. }
        ));
        assert_eq!(devices[0].confirm_by, None);
    }

    /// The example the app checks its side against.
    #[test]
    fn the_approval_number_is_four_digits_both_sides_work_out() {
        let token = "0123456789abcdef";
        assert_eq!(token_hash(token), "9f9f5111f7b27a781f1f1ddde5ebc2dd2b796bfc7365c9c28b548e564176929f");
        assert_eq!(approval_number(&"a".repeat(64), &"b".repeat(64), &token_hash(token)), "0722");
        assert_ne!(approval_number(&"a".repeat(64), &"c".repeat(64), &token_hash(token)), "0722", "another device's key");
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
            pending_endpoint_id: String::new(),
            pending_kind: 0,
            pending_since: None,
            unnamed: false,
            ..Device::default()
        };
        assert!(!remote_on(&[], 100));
        assert!(!remote_on(&[device("k", None, true)], 100), "this machine's app alone");
        assert!(remote_on(&[device("", Some(200), false)], 100), "a code waiting");
        assert!(!remote_on(&[device("", Some(50), false)], 100), "a code expired");
        assert!(!remote_on(&[device("", None, false)], 100), "paired before the tunnel");
        assert!(remote_on(&[device("k", None, false)], 100), "a device with its key");
    }
}
