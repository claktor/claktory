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
use iroh::{Endpoint, RelayConfig, RelayMap, RelayMode, RelayUrl, SecretKey, TransportAddr, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::{Config, RemoteConfig};
pub(crate) use crate::encoding::percent_encode as encode;
use crate::encoding::{hex, percent_decode as decode, unhex};
use crate::factory::{Factory, Inner, RequestError};
use crate::home::Home;
use crate::locking::lock;
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
/// The longest the monitor waits with no app connected and nothing coming
/// due, for a change that didn't wake it (#454).
const IDLE_CHECK: Duration = Duration::from_secs(10);
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
pub(crate) const REFUSED: u32 = 1;
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
    std::fs::rename(partial, path)?;
    // The rename itself on disk too (friend bug #38): without it, a crash
    // or a disk that fails soon after can bring back the file before it, a
    // device paired a minute earlier forgotten.
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
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
    /// When its grace to confirm was to end, in ms since 1970, from
    /// versions before #527 S3 (no deadlines): read, never set or used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_by: Option<i64>,
    /// How it was let in (#527 S3), for Settings → Devices: when, in ms
    /// since 1970, and by what (`ALLOWED_BY_*`): another device (or this
    /// computer's own app) by its id and its name then, the terminal, a
    /// program, the first-device window, or "before" for a device let in
    /// before this was kept. Empty for one let in otherwise (its code and
    /// the owner's account). Never the digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub allowed_by: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub allowed_by_device: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub allowed_by_name: String,
    /// It's the owner's by its Allow: a person compared its digits, on
    /// this computer's own app, on a confirmed device or at a terminal
    /// (#527 S3). Such a device needs no assertion; one let in by a
    /// program or an unconfirmed device does, while this Claktory is
    /// signed in.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_confirmed: bool,
    /// Its Allow's notice waits in Needs you on the person's devices
    /// (RemoteAccess.allow_notices) until one dismisses it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_notice: bool,
    /// When it was last turned away, and why ("other_account": signed in
    /// to another account than this Claktory's, #527 S4), so the person's
    /// other devices see it (a stolen phone, say). Cleared when it next
    /// gets in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turned_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub turned_why: String,
    /// The device waiting with it showed a valid assertion for this
    /// Claktory's account.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending_signed_in: bool,
    /// What its app last said it is, from a Hello that named its system
    /// (Hello.device_kind, platform and client_version): counted, never
    /// named, in the account's check-ins (#119).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub app_kind: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app_platform: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app_version: String,
    /// The person dismissed the "Get notified…" offer on it (area 5a item
    /// 4), until it says its notifications are allowed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub notifications_offer_dismissed: bool,
}

/// Why a device isn't let in while this Claktory's disk fails it (friend bug
/// #38): nothing about it changed, so it isn't told to pair again.
const DISK_TROUBLE: &str = "This Claktory can't use its disk right now, so it can't let this device in. You don't need to add this device again: \
     it connects by itself once the disk works.";

/// What a device found through the account is told while this Claktory is
/// signed out of it (friend bug #38: after a disk failure, say): signing it
/// in again lets the device in as before, where a new code would pair it
/// twice. With SignInRequired, not Unauthorized: the app keeps what it
/// hasn't sent (Unauthorized means removed, and wipes it: #446) and tries
/// again rather than asking for a code.
const SIGNED_OUT: &str = "This Claktory was signed out of your Claktory account, so it can't let this device in. \
     Sign it in again in its Settings → Account, then try again here.";

/// What a device made without a name is called until one uses its code.
const UNNAMED: &str = "New device";
/// The longest name a device takes for itself.
const NAME_CHARS: usize = 60;

#[allow(clippy::trivially_copy_pass_by_ref)] // serde's skip_serializing_if passes a reference
fn is_zero(value: &i32) -> bool {
    *value == 0
}

/// Why a device was last turned away (Device.turned_why).
pub const TURNED_OTHER_ACCOUNT: &str = "other_account";
/// How often that's noted while it keeps trying.
const TURNED_EVERY: i64 = 60_000;

/// What let a device in (Device.allowed_by, PairedDevice.allowed_by).
pub const ALLOWED_BY_DEVICE: &str = "device";
pub const ALLOWED_BY_TERMINAL: &str = "terminal";
pub const ALLOWED_BY_PROGRAM: &str = "program";
pub const ALLOWED_BY_FIRST_DEVICE: &str = "first_device";
pub const ALLOWED_BY_BEFORE: &str = "before";

/// Who allows a waiting device, with its digits compared by the backend
/// (#527 S3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Allower {
    /// A device: its id ("local" for this computer's own app), its name,
    /// and whether it's confirmed as the owner's itself.
    Device { id: String, name: String, confirmed: bool },
    /// A person at a terminal: `factoryd pair`, or `factoryd allow` on one.
    Terminal,
    /// `factoryd allow` from a program, with no terminal.
    Program,
}

impl Allower {
    /// A person compared the digits on a device of the owner's: the
    /// device it lets in is the owner's too.
    fn confirms(&self) -> bool {
        matches!(self, Allower::Device { confirmed: true, .. } | Allower::Terminal)
    }
}

impl Device {
    /// It's the owner's: by an Allow a person made (`allow_confirmed`), or
    /// for `owner`, the account this Claktory is signed in to.
    pub(crate) fn confirmed_for(&self, owner: Option<&str>) -> bool {
        self.local || self.allow_confirmed || owner.is_some_and(|owner| !owner.is_empty() && self.confirmed_as == owner)
    }

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
    let mut devices = devices_as_saved(home)?;
    let now = now_ms();
    for device in &mut devices {
        allowed_before(device, now);
    }
    Ok(devices)
}

/// devices.json as it is on disk, without `allowed_before`.
fn devices_as_saved(home: &Home) -> Result<Vec<Device>, RemoteError> {
    let path = devices_path(home);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| RemoteError::Devices(path, error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

/// At start: writes `allowed_before`'s marks to devices.json, so a device
/// still within its grace when this version started stays allowed after
/// that grace would have ended, even if it doesn't connect meanwhile.
pub(crate) fn record_allowed_before(home: &Home) -> Result<(), RemoteError> {
    let _changing = devices_lock();
    let saved = devices_as_saved(home)?;
    let marked = devices(home)?;
    if marked != saved {
        save_devices(home, &marked)?;
        tracing::info!("devices allowed before Allows were recorded count as allowed");
    }
    Ok(())
}

/// A device let in with its code before Allows were recorded (#527 S3),
/// and not confirmed by an assertion: every Allow since #352 P4 compared
/// its digits, so it's the owner's by its Allow, and no deadline turns it
/// away. Not one whose grace had already run out, which versions before
/// were turning away (Fugo 40892): it still confirms. Every way a device
/// is let in now records how (`allowed_by` or `confirmed_as`), so this
/// only ever finds devices from before (`every_way_in_is_recorded`).
/// Written at start (`record_allowed_before`), and with any change.
fn allowed_before(device: &mut Device, now: i64) {
    if !device.local
        && !device.discovered
        && !device.endpoint_id.is_empty()
        && device.allowed_by.is_empty()
        && device.confirmed_as.is_empty()
        && !device.allow_confirmed
        && device.confirm_by.is_none_or(|by| by > now)
    {
        device.allowed_by = ALLOWED_BY_BEFORE.to_owned();
        device.allow_confirmed = true;
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
    let config = Config::parse(&text, home.root()).map_err(|error| RemoteError::Config(path.clone(), error.to_string()))?;
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

/// Where a pairing code's QR code sends a phone's camera: Claktory's page,
/// which Claktory claims as a Universal Link and App Link (only this path),
/// so the camera opens the app, or the page offers it in the store.
pub const PAIR_PAGE: &str = "https://claktory.com/pair";

/// Whether QR codes hold the link (#352 P6, step 2). Off until the page,
/// its headers and its Universal Link and App Link files are live and
/// checked, and phone apps that read the link are out: until then the page
/// could answer with anything, a redirect that takes the code along
/// included, and an older app can't read the link. Off, `pairing_link` is
/// empty and QR codes hold the `factory://` code.
const PAIR_LINKS: bool = false;

/// A pairing code as a link for its QR code (PairingCode.link), once
/// PAIR_LINKS is on: `PAIR_PAGE#` and the code's query, unchanged, so only a
/// device ever reads it (a browser never sends a fragment to the page's
/// server). Empty for anything that isn't a `factory://?` code.
pub fn pairing_link(code: &str) -> String {
    if PAIR_LINKS { link_of(code) } else { String::new() }
}

fn link_of(code: &str) -> String {
    code.strip_prefix("factory://?").map(|query| format!("{PAIR_PAGE}#{query}")).unwrap_or_default()
}

/// A numbered version's line, its first two numbers: v0.1.11 and
/// v0.1.12-dev.3 are both on (0, 1). A release stamped `--incompatible`
/// starts a new line (v0.1.7 → v0.2.0), and only an app and a backend on one
/// line work together, as the app's own check reads it (compatibility.dart's
/// versionLine). None for what isn't numbered ("dev", the date stamps from
/// before v0.1.0).
fn version_line(version: &str) -> Option<(u64, u64)> {
    let numbered = version.strip_prefix('v')?;
    let numbered = numbered.split_once('-').map_or(numbered, |(numbers, _)| numbers);
    let parts: Vec<&str> = numbered.split('.').collect();
    let [major, minor, patch] = parts[..] else { return None };
    if [major, minor, patch].iter().any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit())) {
        return None;
    }
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Whether an app at `app` can't work with the backend `offered`: both
/// numbered, on different lines.
fn leaves_behind(app: &str, offered: &str) -> bool {
    matches!((version_line(app), version_line(offered)), (Some(app), Some(offered)) if app != offered)
}

/// What a Claktory is called and its color, as devices show it.
pub struct Identity {
    pub name: String,
    pub color: String,
}

/// This Claktory's identity from config.json, away from the running Factory
/// (defaults when the file is missing or broken).
fn identity(home: &Home) -> Identity {
    let config =
        std::fs::read_to_string(home.config_path()).ok().and_then(|text| Config::parse(&text, home.root()).ok()).unwrap_or_default();
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

/// How many wrong numbers turn a waiting device away (#352 P4).
const WRONG_NUMBERS: u32 = 3;

/// A waiting device's name as the person's devices and terminal show it
/// (#352 P4, Fugo): no control characters, nor the invisible ones that
/// reorder or hide text (bidi controls, zero widths, the BOM), at most 40
/// characters. A device found through the account chose it itself.
pub fn shown_name(name: &str) -> String {
    let name = crate::config::clean_name(name).split_whitespace().collect::<Vec<_>>().join(" ");
    let name = name.as_str();
    if name.chars().count() <= 40 { name.to_owned() } else { format!("{}…", name.chars().take(39).collect::<String>().trim_end()) }
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
/// isn't waiting). With the devices an allowed one replaced (`replace_same_key`).
fn settle(home: &Home, device: &str, ended: Result<&Allower, Ended>) -> Result<(Device, Vec<Device>), RemoteError> {
    let _changing = devices_lock();
    let mut devices = devices(home)?;
    let now = now_ms();
    let waiting = |known: &Device| match ended {
        Ok(_) | Err(Ended::Denied) => known.is_pending(now),
        Err(_) => known.endpoint_id.is_empty() && !known.local,
    };
    let Some(index) = devices.iter().position(|known| (known.id == device || known.name == device) && waiting(known)) else {
        return Err(RemoteError::NotWaiting(device.to_owned()));
    };
    let allower = match ended {
        Ok(allower) => allower,
        Err(_) => {
            let removed = devices.remove(index);
            save_devices(home, &devices)?;
            return Ok((removed, Vec::new()));
        }
    };
    let allowed = &mut devices[index];
    allowed.allowed_ms = Some(now);
    (allowed.allowed_by, allowed.allowed_by_device, allowed.allowed_by_name) = match allower {
        Allower::Device { id, name, .. } => (ALLOWED_BY_DEVICE.to_owned(), id.clone(), name.clone()),
        Allower::Terminal => (ALLOWED_BY_TERMINAL.to_owned(), String::new(), String::new()),
        Allower::Program => (ALLOWED_BY_PROGRAM.to_owned(), String::new(), String::new()),
    };
    allowed.allow_confirmed = allower.confirms();
    allowed.allow_notice = true;
    allowed.endpoint_id = std::mem::take(&mut allowed.pending_endpoint_id);
    allowed.pending_kind = 0;
    allowed.pending_since = None;
    allowed.pending_signed_in = false;
    allowed.code_expires = None;
    let (index, replaced) = replace_same_key(&mut devices, index);
    let allowed = devices[index].clone();
    save_devices(home, &devices)?;
    Ok((allowed, replaced))
}

/// Lets in the device waiting with this device's code (by id or name),
/// allowed by a person at a terminal who compared its digits (`factoryd
/// pair`). A device paired before with the same key is gone: it was this
/// one.
pub fn approve(home: &Home, device: &str) -> Result<Device, RemoteError> {
    approve_by(home, device, &Allower::Terminal)
}

/// `approve`, by `allower`: tests stand in for the app or a program with it.
pub fn approve_by(home: &Home, device: &str, allower: &Allower) -> Result<Device, RemoteError> {
    settle(home, device, Ok(allower)).map(|(allowed, _)| allowed)
}

/// Turns away the device waiting with this device's code, and forgets it.
pub fn deny(home: &Home, device: &str) -> Result<Device, RemoteError> {
    settle(home, device, Err(Ended::Denied)).map(|(denied, _)| denied)
}

/// Ends a code that hasn't let a device in, and forgets its device.
pub fn cancel(home: &Home, device: &str) -> Result<Device, RemoteError> {
    settle(home, device, Err(Ended::Cancelled)).map(|(ended, _)| ended)
}

/// The device waiting under `device` with the key `endpoint_id` takes back
/// its own request (#603): a code's goes back to waiting for a device to
/// use the code, while it runs; one that came through the account is
/// forgotten. Err(NotWaiting) when no request is waiting so.
pub fn withdraw(home: &Home, device: &str, endpoint_id: &str) -> Result<(), RemoteError> {
    let _changing = devices_lock();
    let mut devices = devices(home)?;
    let now = now_ms();
    let waiting = |known: &Device| known.id == device && known.is_pending(now) && known.pending_endpoint_id == endpoint_id;
    let Some(index) = devices.iter().position(waiting) else {
        return Err(RemoteError::NotWaiting(device.to_owned()));
    };
    if devices[index].discovered {
        devices.remove(index);
    } else {
        let request = &mut devices[index];
        request.pending_endpoint_id.clear();
        request.pending_kind = 0;
        request.pending_since = None;
        request.pending_signed_in = false;
    }
    save_devices(home, &devices)
}

/// Drops every other device that connects with the key `devices[keep]` was
/// just let in with (friend bug #38): a key is one device, so one paired
/// again (with a new code, or by a code after it came through the account)
/// is listed once, as the pairing the person just allowed, never twice. Only
/// ever on the person's Allow or a device's own code and account: holding a
/// key never brings back a device that was removed. Returns where
/// `devices[keep]` is now, and the devices dropped.
fn replace_same_key(devices: &mut Vec<Device>, keep: usize) -> (usize, Vec<Device>) {
    let (id, key) = (devices[keep].id.clone(), devices[keep].endpoint_id.clone());
    if key.is_empty() || devices[keep].local {
        return (keep, Vec::new());
    }
    let (replaced, kept): (Vec<Device>, Vec<Device>) =
        devices.drain(..).partition(|device| device.id != id && !device.local && device.endpoint_id == key);
    *devices = kept;
    let keep = devices.iter().position(|device| device.id == id).expect("the device let in is kept");
    (keep, replaced)
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
            let notifications_offer_dismissed = local.is_some_and(|index| devices[index].notifications_offer_dismissed);
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
                notifications_offer_dismissed,
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
    /// Who may use the direct-message link (`dm_link`): only they get its
    /// ALPN, and only it.
    dms: Arc<crate::dm_link::Admits>,
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
        if connection.alpn() == crate::dm_link::ALPN_DM {
            return self.dms.admits(&connection.remote_id().to_string());
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
            // A file that can't be read for now (a failing disk, friend bug
            // #38) isn't taken as "not paired": the Hello that follows is
            // still checked, and tells the device why it can't come in.
            Err(error) => {
                tracing::error!(%error, "could not read the paired devices");
                true
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
    /// With config.json's relays it was started with.
    remote: Option<(Vec<String>, Endpoint, tokio::task::JoinHandle<()>)>,
    /// The remote endpoint's relays now (#450), its token on Claktory's.
    relays: Vec<RelayConfig>,
    /// Relays it no longer uses, kept until then (`RELAY_GRACE`).
    retiring: Vec<(RelayUrl, Instant)>,
    /// What failed to start last, so it isn't tried every second.
    failed: Option<Wanted>,
}

/// Why a session is closed, when it's turned away rather than unpaired:
/// set before its `closed` is notified, told to it before it closes.
pub(crate) type Refusal = Arc<Mutex<Option<v1::Error>>>;

/// One of a device's open sessions.
struct Session {
    closed: Arc<tokio::sync::Notify>,
    endpoint_id: String,
    /// The account it came in through, for a device found through the
    /// account (`Device::discovered`); empty for any other (a code's, this
    /// machine's app), which the account's sign-in doesn't hold open.
    account: String,
    /// Its Hello's platform, to name it if it's turned away.
    platform: String,
    /// Why it's closed, when it's turned away rather than unpaired.
    refusal: Refusal,
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
    /// Local relays and address lookup in place of the internet's; tests'.
    network: Mutex<Option<Network>>,
    /// How long a relay no longer used is kept; tests make it shorter.
    relay_grace: Mutex<Option<Duration>>,
    /// Notified when a waiting device is allowed or its code ends.
    pairing: tokio::sync::Notify,
    /// How each code that let no device in ended, by its device's id, for
    /// the device waiting with it.
    ended: Mutex<HashMap<String, Ended>>,
    /// How many wrong numbers each waiting device was allowed with, by its
    /// id (#352 P4): the third turns it away.
    wrong_numbers: Mutex<HashMap<String, u32>>,
    /// Connections of devices waiting to be allowed, or being told they
    /// weren't: remote access stays on for them until they've heard.
    waiting: AtomicU64,
    /// The tokens of agents' pairings still waiting for a device, by
    /// pairing id (`extensions::devices`): only in memory, never on disk.
    codes: Mutex<HashMap<String, String>>,
    /// Notified whenever devices may have changed, for agents' pairings to
    /// show it (`extensions::devices::monitor`).
    pub(crate) changed: Arc<tokio::sync::Notify>,
    /// Wakes the tunnel's monitor: something it follows may have changed
    /// (`refresh_remote`, a device waiting).
    wake: Arc<tokio::sync::Notify>,
    /// This Claktory is signed in to an account: the gate lets in keys it
    /// doesn't know, for the person's devices to find it (see `admit`).
    account_open: Arc<AtomicBool>,
    /// The port Claktory's own listener has (7420), which devices may not
    /// forward to; 0 until it listens.
    listener_port: AtomicU64,
    /// Pipes to other Claktories, their transport's (`pipe_link`).
    pub(crate) pipes: crate::pipe_link::State,
    /// Told whenever a session starts or ends, for each device connection's
    /// keep-alives to follow whether its key is let in (`pace_admitted`).
    sessions_changed: tokio::sync::watch::Sender<()>,
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

/// Who asks to allow a device (ApproveDevice): its device id
/// (`LOCAL_DEVICE` for this computer), whether it's `factoryd`'s own
/// commands rather than an app (`factoryd allow`), and, for those, whether
/// they ran on a terminal (ApproveDevice.from_terminal).
pub struct Asker {
    pub device: String,
    pub tool: bool,
    pub from_terminal: bool,
}

/// What became of a device's Hello with its token.
pub(crate) enum Authorized {
    Connected(Connected),
    /// It used a pairing code first, or came through the account, and waits
    /// for the person to allow it.
    Pending(Device),
    Refused,
    /// Turned away for a reason it can act on: the Error to answer with
    /// (its code and what to tell it).
    Turned(v1::Error),
}

/// What a device's Hello says of who it is (`Factory::authorize`).
pub(crate) struct Knock<'a> {
    /// Its pairing code's token, or this machine's app's; empty for a
    /// device found through the account.
    pub token: &'a str,
    /// A device assertion from the account service (Hello.assertion): it's
    /// signed in to the person's account. Empty without one.
    pub assertion: &'a str,
    pub kind: v1::DeviceKind,
    /// Its own name (Hello.device_name), for a device that names itself.
    pub name: &'a str,
    /// Its system and app version (Hello.platform, client_version); the
    /// platform is empty from apps before it, and from tools.
    pub platform: &'a str,
    pub app_version: &'a str,
}

/// How long after this Claktory signs in its first device may join through
/// the account alone, without Allow: the fresh server's case.
pub const FIRST_DEVICE_WINDOW: Duration = Duration::from_secs(30 * 60);

/// This Claktory's account, for `admit`.
pub(crate) struct Owner {
    pub account_id: String,
    /// When it signed in, in ms since 1970; 0 if before that was kept.
    pub signed_in_ms: i64,
    /// This Claktory's name, for what a device is told.
    pub name: String,
    /// The email it's signed in with, masked as the app shows it
    /// ("m…@example.com"), for what a device is told; empty if unknown.
    pub email: String,
}

/// What `admit` decided, by the device's place in the list.
#[derive(Debug)]
enum Admission {
    Connected(usize),
    Pending(usize),
    /// The same, taken out of the list once it's saved.
    Found(Device),
    Waits(Device),
    Turned(v1::ErrorCode, String),
    /// Turned away as signed in to another account, having been let in
    /// here before: ERROR_CODE_OTHER_ACCOUNT, with
    /// Error.other_account_before.
    OtherAccountNow(String),
    /// Turned away to sign in, a device this Claktory recognizes (paired
    /// here, not removed): ERROR_CODE_SIGN_IN_REQUIRED, with where to sign
    /// in (Error.account_service, account_email; Fugo 47717).
    SignInKnown(String),
    Unknown,
}

/// What an assertion that doesn't count means to the device showing it.
fn refusal(refused: crate::account::AssertionRefusal) -> (v1::ErrorCode, String) {
    use crate::account::AssertionRefusal as Refused;
    match refused {
        Refused::Expired => (v1::ErrorCode::SignInRequired, "This device's sign-in ran out. Connect again to sign in afresh.".to_owned()),
        Refused::OtherAccount => (v1::ErrorCode::OtherAccount, other_account(false, "", "This Claktory")),
        Refused::NotSignedIn => (
            v1::ErrorCode::Unauthorized,
            "This Claktory isn't signed in to an account, so it can't be found this way. Add this device with a code from its Settings → Devices.".to_owned(),
        ),
        Refused::Invalid | Refused::OtherKey => {
            (v1::ErrorCode::Unauthorized, "This device's sign-in couldn't be checked. Sign out and in again on it.".to_owned())
        }
    }
}

/// The characters the app shows to check against (Trish 41556: "Check
/// that it matches what … showed") for this Claktory's pairing codes
/// (MC7): the first 16 hex digits of its tunnel key, in fours, as the app's
/// pairingFingerprint writes them, so the two match by eye.
pub fn code_fingerprint(home: &Home) -> String {
    let key = secret_key(home).map(|key| key.public().to_string()).unwrap_or_default();
    fingerprint_of(&key)
}

fn fingerprint_of(key: &str) -> String {
    let short: Vec<char> = key.chars().take(16).collect();
    short.chunks(4).map(|chunk| chunk.iter().collect::<String>()).collect::<Vec<_>>().join(" ")
}

/// What a device is called by its Hello's platform.
fn device_word(platform: &str) -> &'static str {
    match platform {
        "macos" => "Mac",
        "ios" => "iPhone",
        "ipados" => "iPad",
        "android" => "phone",
        _ => "device",
    }
}

/// Why a device let in here through the account before is turned away
/// once this Claktory, `name`, is signed in to another account, `email`
/// (masked; row 713, Fugo 50367, Trish 50372): it's this Claktory that
/// changed, not the device.
fn claktory_switched(platform: &str, name: &str, email: &str) -> String {
    let device = device_word(platform);
    // Kept whole at a narrow width: "m…" never apart from its "@example.com"
    // (Zucchero's row 727 NIT).
    let email = email.replace("…@", "…\u{2060}@");
    let email = email.as_str();
    if email.is_empty() {
        format!(
            "{name} is now signed in to a different account, so it no longer lets this {device} in. To connect again, sign in here with the \
             account {name} uses, or use a new code."
        )
    } else {
        format!(
            "{name} is now signed in as {email}, a different account, so it no longer lets this {device} in. To connect again, sign in here \
             with {email}, or use a new code."
        )
    }
}

/// Why a device signed in to another account than this Claktory's is
/// turned away (#527 S4): `known`, one let in here before; `platform`, its
/// Hello's, to name it; `name`, this Claktory's.
fn other_account(known: bool, platform: &str, name: &str) -> String {
    let device = device_word(platform);
    if known {
        format!("This {device} is now signed in to a different Claktory account. Sign in with the account {name} uses to connect again.")
    } else {
        format!("So it can't let this {device} in. To work with someone else's agents, open a pipe with them.")
    }
}

/// A device's own name, as it gives it: without what could hide or reorder
/// text (`config::clean_name`), whitespace squeezed, at most `NAME_CHARS`.
fn own_name(name: &str) -> String {
    crate::config::clean_name(name)
        .split_whitespace().collect::<Vec<_>>().join(" ").chars().take(NAME_CHARS).collect()
}

/// A device whose code is `knock`'s, bound to `peer`'s key, let in and
/// confirmed by an Allow a person made, or this computer's own app.
fn bound_and_allowed(devices: &[Device], knock: &Knock<'_>, peer: &Peer) -> bool {
    let digest = token_hash(knock.token);
    !knock.token.is_empty()
        && !peer.endpoint_id.is_empty()
        && devices.iter().any(|device| {
            device.token_sha256 == digest
                && device.endpoint_id == peer.endpoint_id
                && device.pending_endpoint_id.is_empty()
                && (device.allow_confirmed || device.local)
        })
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
    // An assertion that ran out, from a device let in here before by its
    // key and confirmed by an Allow a person made (or this computer's own
    // app): it needs none, so it's as if it sent none (Fugo 40892), and a
    // Claktory whose clock is off doesn't lock it out. Only Expired
    // (`refusal` gives SignInRequired for that alone): another account,
    // another key or a bad signature still refuse.
    let expired = matches!(&shown, Some(Err((v1::ErrorCode::SignInRequired, _))));
    let shown = if expired && bound_and_allowed(devices, knock, peer) { None } else { shown };
    let signed_in = matches!(&shown, Some(Ok(_)));
    // While this Claktory is signed in (#527 S4): an assertion for another
    // account is turned away, before and after Allow (a friend's device:
    // pipes are for that); one that doesn't verify is refused as it is,
    // never taken for none.
    if owner.is_some()
        && !peer.endpoint_id.is_empty()
        && let Some(Err((code, reason))) = &shown
    {
        if *code == v1::ErrorCode::OtherAccount
            && let Some(owner) = owner
        {
            let (mut known, mut switched) = (false, false);
            for device in devices
                .iter_mut()
                .filter(|device| !device.local && !device.endpoint_id.is_empty() && device.endpoint_id == peer.endpoint_id)
            {
                known = true;
                // Confirmed for another account than this Claktory's now:
                // it's this Claktory that changed accounts (row 713).
                switched |= !device.confirmed_as.is_empty() && device.confirmed_as != owner.account_id;
                // Noted once a minute at most, however often it retries.
                if device.turned_why != TURNED_OTHER_ACCOUNT || device.turned_ms.is_none_or(|at| now - at >= TURNED_EVERY) {
                    device.turned_ms = Some(now);
                    device.turned_why = TURNED_OTHER_ACCOUNT.to_owned();
                }
            }
            let reason = if switched {
                claktory_switched(knock.platform, &owner.name, &owner.email)
            } else {
                other_account(known, knock.platform, &owner.name)
            };
            return if known { Admission::OtherAccountNow(reason) } else { Admission::Turned(*code, reason) };
        }
        // With a code, never Unauthorized: that says it was removed, and
        // the app would wipe what it hasn't sent (#446).
        let code = if knock.token.is_empty() || *code != v1::ErrorCode::Unauthorized { *code } else { v1::ErrorCode::SignInRequired };
        return Admission::Turned(code, reason.clone());
    }
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
                Some(Err(_)) if owner.is_none() => Admission::SignInKnown(SIGNED_OUT.to_owned()),
                Some(Err((code, reason))) => Admission::Turned(code, reason),
                _ => Admission::SignInKnown("Sign in to your Claktory account on this device to connect.".to_owned()),
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
                        device.allowed_ms = Some(now);
                        device.allowed_by = ALLOWED_BY_FIRST_DEVICE.to_owned();
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
    // Is it the owner's? Its assertion says so, or an Allow a person made
    // (#527 S3: no deadline); one let in otherwise (by a program, or a
    // device not confirmed itself) shows the owner's assertion first.
    if let Some(owner) = owner
        && !device.local
    {
        if signed_in {
            device.confirmed_as.clone_from(&owner.account_id);
        } else if !device.confirmed_for(Some(&owner.account_id)) {
            return Admission::SignInKnown("Sign in to your Claktory account on this device to connect.".to_owned());
        }
    }
    // Versions before kept a deadline: gone once it's confirmed. One past
    // it stays so, and keeps confirming (`allowed_before`).
    if device.confirmed_for(owner.map(|owner| owner.account_id.as_str())) {
        device.confirm_by = None;
    }
    if !peer.endpoint_id.is_empty() {
        device.endpoint_id.clone_from(&peer.endpoint_id);
    }
    device.last_seen = Some(now);
    device.turned_ms = None;
    device.turned_why.clear();
    // Only an app says its system: a tool on the loopback port (factoryd's
    // own commands, QA) leaves this computer's app's as they were.
    if !knock.platform.is_empty() {
        device.app_kind = knock.kind as i32;
        device.app_platform = knock.platform.chars().take(40).collect();
        device.app_version = knock.app_version.chars().take(40).collect();
    }
    device.last_address = peer.address.map(|address| address.ip().to_canonical().to_string()).unwrap_or_default();
    Admission::Connected(index)
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
        if let Err(error) = record_allowed_before(home) {
            tracing::warn!(%error, "could not record which devices were allowed before Allows were recorded");
        }
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
    /// This machine's own app (not one of `devices`) dismissed the "Get
    /// notified…" offer.
    local_offer_dismissed: bool,
}

/// Claktory's own relay (iroh-relay), the first of the relays the tunnel
/// uses unless config.json names others. They used to be n0's: devices
/// paired then, whose codes name n0's relays, find this Claktory on these
/// through the address lookup. Written exactly as iroh compares relays (as
/// written, a trailing slash added): the apps home on the same string.
pub const DEFAULT_RELAY: &str = "https://relay.claktory.com/";

/// The relays the tunnel uses unless config.json names others: Claktory's
/// own, and n0's US East one in case it's down (an endpoint homes on the
/// fastest it reaches, so Europe stays on Claktory's). Not n0's Frankfurt
/// one: about as close as Claktory's from Europe, it would bring back the
/// rate limits. n0's as iroh writes it, so it's the relay devices that home
/// there know.
pub fn default_relays() -> Vec<String> {
    vec![DEFAULT_RELAY.to_owned(), iroh::defaults::prod::default_na_east_relay().url.to_string()]
}

/// n0's public relays (iroh's default ones): what a Claktory that isn't
/// signed in to claktory.com uses, and its devices with it, since
/// Claktory's relay forwards only to or from a signed-in Claktory (#450).
pub fn public_relays() -> Vec<String> {
    let urls: Vec<RelayUrl> = iroh::defaults::prod::default_relay_map().urls();
    urls.iter().map(ToString::to_string).collect()
}

/// `public_relays`, as iroh takes them: where moves go, since neither end
/// of one has a token for Claktory's relay.
pub(crate) fn public_relay_map() -> RelayMap {
    iroh::defaults::prod::default_relay_map()
}

/// Whether `url` is Claktory's own relay, however it's written (a dot
/// after the host, no slash after it), and only over https: the one relay
/// the tunnel gives its relay token to.
fn is_claktorys_relay(url: &RelayUrl) -> bool {
    let ours: RelayUrl = DEFAULT_RELAY.parse().expect("Claktory's relay URL");
    url.scheme() == ours.scheme() && url.port() == ours.port() && url.host_str().map(|host| host.trim_end_matches('.')) == ours.host_str()
}

/// How long the tunnel keeps a relay it no longer uses (#450), so devices
/// that learn the new ones from the Workspace still reach it meanwhile.
const RELAY_GRACE: Duration = Duration::from_secs(60);

/// What tests put in place of the internet (`set_tunnel_network`): the
/// default relays (the first standing for Claktory's, which gets the relay
/// token), n0's public ones, and what sets up the address lookup
/// `presets::N0` would.
#[derive(Clone)]
struct Network {
    relays: Vec<String>,
    /// None: the default ones stand for them too.
    public: Option<Vec<String>>,
    lookup: Arc<dyn Fn(Builder) -> Builder + Send + Sync>,
}

/// One of a device's sessions; it counts as connected while this lives.
pub(crate) struct Connected {
    pub device: Device,
    /// Notified when the device is unpaired: the session closes.
    pub closed: Arc<tokio::sync::Notify>,
    /// Set before `closed` when it's turned away rather than unpaired (row
    /// 713): the refusal it's told first.
    pub refusal: Refusal,
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
        factory.remote.sessions_changed.send_replace(());
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
        let owner = crate::account::owner(self, &account).map(|owner| Owner {
            account_id: owner.account_id,
            signed_in_ms: owner.signed_in_ms,
            name: self.identity().name,
            email: self.account.where_to_sign_in(&account).1,
        });
        // What its assertion shows, if it brought one through the tunnel.
        let shown = (!knock.assertion.is_empty() && !peer.endpoint_id.is_empty()).then(|| {
            crate::account::check_assertion(self, &account, knock.assertion, &peer.endpoint_id)
                .map(|assertion| assertion.account_id)
                .map_err(refusal)
        });
        let proved = shown.as_ref().and_then(|shown| shown.as_ref().ok().cloned());
        let mut replaced = Vec::new();
        let mut noted = false;
        let changing = devices_lock();
        let admission = match devices(&self.home) {
            Ok(mut devices) => {
                // Let in with this key before (or this machine's app): it
                // stays in when only its last connection can't be noted.
                let bound = |devices: &[Device], index: usize| {
                    devices
                        .get(index)
                        .is_some_and(|device| device.local || (!peer.endpoint_id.is_empty() && device.endpoint_id == peer.endpoint_id))
                };
                let before: Vec<bool> = (0..devices.len()).map(|index| bound(&devices, index)).collect();
                let unchanged = devices.clone();
                let mut admission = admit(&mut devices, knock, peer, owner.as_ref(), shown, now);
                // A device of the person's turned away as another account's:
                // noted, for their other devices to see (#527 S4).
                if matches!(admission, Admission::OtherAccountNow(_)) && devices != unchanged {
                    match save_devices(&self.home, &devices) {
                        Ok(()) => noted = true,
                        Err(error) => tracing::warn!(%error, "could not note that a device was turned away"),
                    }
                }
                // A device just bound to its key by its code and account
                // replaces any other with that key: the same device.
                if let Admission::Connected(index) = admission
                    && !before.get(index).copied().unwrap_or(false)
                {
                    let (index, dropped) = replace_same_key(&mut devices, index);
                    replaced = dropped;
                    admission = Admission::Connected(index);
                }
                if matches!(admission, Admission::Connected(_) | Admission::Pending(_))
                    && let Err(error) = save_devices(&self.home, &devices)
                {
                    // A disk that can't be written (read-only, failing:
                    // friend bug #38) doesn't unpair anyone: a device let
                    // in before comes in, only when it came isn't noted. One that
                    // would be new is told why it can't be, not that it
                    // isn't paired.
                    replaced.clear();
                    admission = match admission {
                        Admission::Connected(index) if before.get(index).copied().unwrap_or(false) => {
                            tracing::warn!(%error, "could not note when a device connected; it's let in all the same");
                            admission
                        }
                        _ => {
                            tracing::warn!(%error, "could not save a device's pairing");
                            Admission::Turned(v1::ErrorCode::Internal, DISK_TROUBLE.to_owned())
                        }
                    };
                }
                match admission {
                    Admission::Connected(index) => Admission::Found(devices.swap_remove(index)),
                    Admission::Pending(index) => Admission::Waits(devices.swap_remove(index)),
                    other => other,
                }
            }
            // Only a device known here (as last shown, or this machine's
            // app) hears it's the disk; any other key, the usual refusal.
            Err(error) => {
                tracing::error!(%error, "could not read the paired devices");
                let known = peer.loopback
                    || from_this_machine(peer.address)
                    || lock(&self.remote.shown).devices.iter().any(|device| {
                        !peer.endpoint_id.is_empty()
                            && (device.endpoint_id == peer.endpoint_id || device.pending_endpoint_id == peer.endpoint_id)
                    });
                if known { Admission::Turned(v1::ErrorCode::Internal, DISK_TROUBLE.to_owned()) } else { Admission::Unknown }
            }
        };
        drop(changing);
        if !replaced.is_empty() {
            tracing::info!(replaced = replaced.len(), "a device paired again replaced its earlier pairing");
            // Their key is the one coming in: its tunnel stays.
            self.forget_devices(&replaced, false).await;
        }
        let device = match admission {
            Admission::Found(device) => device,
            Admission::Waits(device) => {
                tracing::info!(device = %device.id, endpoint = %peer.endpoint_id, discovered = device.discovered, "a device waits to be allowed");
                // An earlier code's device by the same id ended some way: not this one.
                lock(&self.remote.ended).remove(&device.id);
                self.refresh_remote();
                return Authorized::Pending(device);
            }
            Admission::Turned(code, message) => {
                return self.turn_away(peer, noted, v1::Error { code: code.into(), message, ..Default::default() }).await;
            }
            Admission::OtherAccountNow(message) => {
                let refusal =
                    v1::Error { code: v1::ErrorCode::OtherAccount.into(), message, other_account_before: true, ..Default::default() };
                return self.turn_away(peer, noted, refusal).await;
            }
            Admission::SignInKnown(message) => {
                let (account_service, account_email) = self.account.where_to_sign_in(&account);
                let refusal = v1::Error { code: v1::ErrorCode::SignInRequired.into(), message, account_service, account_email, ..Default::default() };
                return self.turn_away(peer, noted, refusal).await;
            }
            Admission::Unknown | Admission::Connected(_) | Admission::Pending(_) => {
                // The loopback port's refusals are logged where they're known.
                if !peer.endpoint_id.is_empty() {
                    tracing::warn!(endpoint = %peer.endpoint_id, "refused a device that isn't paired");
                }
                self.pause_after_refusal(peer).await;
                return Authorized::Refused;
            }
        };
        let closed = Arc::new(tokio::sync::Notify::new());
        let refusal = Arc::new(Mutex::new(None));
        // Found through the account, it's in only while the account it
        // proved is this Claktory's (row 713, Fugo 49686): see `recheck`.
        let account = if device.discovered { proved.unwrap_or_else(|| device.confirmed_as.clone()) } else { String::new() };
        let session = Session {
            closed: closed.clone(),
            endpoint_id: peer.endpoint_id.clone(),
            account,
            platform: knock.platform.to_owned(),
            refusal: refusal.clone(),
        };
        lock(&self.remote.connected).entry(device.id.clone()).or_default().push(session);
        self.remote.sessions_changed.send_replace(());
        self.refresh_remote();
        Authorized::Connected(Connected { device, closed, refusal, factory: Arc::downgrade(self) })
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
                    .any(|device| device.id == asker && device.confirmed_for(Some(&owner.account_id)));
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

    /// A sign-in made on a paired device (AccountVerify) confirms that
    /// device as the owner's (#365 R5): typing the emailed code proved the
    /// email as a device's own sign-in does. Only the device whose id is
    /// `asker` and whose bound key is the one its connection showed,
    /// `endpoint_id`, and only for `account_id`, the account this Claktory
    /// was just signed in to. Nothing for this computer's app, a device
    /// waiting to be allowed, or a key that doesn't match.
    pub(crate) fn confirm_signed_in_device(&self, asker: &str, endpoint_id: &str, account_id: &str) {
        if asker == LOCAL_DEVICE || endpoint_id.is_empty() || account_id.is_empty() {
            return;
        }
        let confirm = || -> Result<bool, RemoteError> {
            let _changing = devices_lock();
            let mut devices = devices(&self.home)?;
            let Some(device) = devices.iter_mut().find(|device| {
                device.id == asker && !device.local && device.endpoint_id == endpoint_id && device.pending_endpoint_id.is_empty()
            }) else {
                return Ok(false);
            };
            device.confirmed_as = account_id.to_owned();
            device.confirm_by = None;
            save_devices(&self.home, &devices)?;
            Ok(true)
        };
        match confirm() {
            Ok(true) => {
                tracing::info!(device = %asker, "a device signed in on itself and was confirmed as the owner's");
                self.refresh_remote();
            }
            Ok(false) => {}
            Err(error) => tracing::error!(%error, device = %asker, "could not confirm a device that signed in"),
        }
    }

    /// Turns `peer` away with `refusal`, once a second after its last
    /// refusal; `noted`, its devices.json changed, for the app to see.
    async fn turn_away(&self, peer: &Peer, noted: bool, refusal: v1::Error) -> Authorized {
        tracing::warn!(endpoint = %peer.endpoint_id, code = ?refusal.code(), "turned a device away");
        if noted {
            self.refresh_remote();
        }
        self.pause_after_refusal(peer).await;
        Authorized::Turned(refusal)
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
        self.remote.wake.notify_one();
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

    /// Allows the device waiting with this device's code, from the app,
    /// with the four digits the person read off it (#352 P4, Fugo): a wrong
    /// number is refused, and the third wrong one turns the device away.
    pub async fn approve_device(&self, asker: &Asker, id: &str, number: &str) -> Result<(), RequestError> {
        // As the devices file has it now, not as last shown: a code that
        // just ran out isn't waiting any more.
        let now = now_ms();
        let known = devices(&self.home).map_err(|error| RequestError::Remote(error.to_string()))?;
        let allower = self.allower(asker, &known);
        let Some(device) = known.into_iter().find(|device| device.id == id && device.is_pending(now)) else {
            return Err(RequestError::UnknownDevice(id.to_owned()));
        };
        let endpoint = lock(&self.remote.shown).endpoint_id.clone();
        let (name, shows) = (shown_name(&device.name), approval_number(&endpoint, &device.pending_endpoint_id, &device.token_sha256));
        let number = number.trim();
        if number.is_empty() {
            return Err(RequestError::Remote(format!("Type the 4 digits {name} shows to allow it.")));
        }
        if number != shows {
            let wrong = {
                let mut wrong = lock(&self.remote.wrong_numbers);
                let count = wrong.entry(id.to_owned()).or_default();
                *count += 1;
                *count
            };
            tracing::warn!(device = %id, wrong, "a device was allowed with a wrong number");
            if wrong >= WRONG_NUMBERS {
                lock(&self.remote.wrong_numbers).remove(id);
                self.deny_device(id).await?;
                return Err(RequestError::Remote(format!(
                    "That's not the number {name} shows, a third time, so it was turned away. To add it, make a new code."
                )));
            }
            let left = WRONG_NUMBERS - wrong;
            let tries = if left == 1 { "1 try left".to_owned() } else { format!("{left} tries left") };
            return Err(RequestError::Remote(format!("That's not the number {name} shows. {tries}.")));
        }
        lock(&self.remote.wrong_numbers).remove(id);
        let replaced = match settle(&self.home, id, Ok(&allower)) {
            Ok((device, replaced)) => {
                tracing::info!(device = %device.id, by = %device.allowed_by, confirmed = device.allow_confirmed, replaced = replaced.len(), "a device was allowed");
                replaced
            }
            Err(RemoteError::NotWaiting(id)) => return Err(RequestError::UnknownDevice(id)),
            Err(error) => return Err(RequestError::Remote(error.to_string())),
        };
        self.remote.pairing.notify_waiters();
        // Their key is the one just let in: its tunnel stays.
        self.forget_devices(&replaced, false).await;
        self.refresh_remote();
        Ok(())
    }

    /// Who allows a device, from what asked (#527 S3): this computer's
    /// own app, or another device as `devices` has it, is a device;
    /// `factoryd allow` is the terminal when it ran on one, else a program.
    fn allower(&self, asker: &Asker, devices: &[Device]) -> Allower {
        if asker.device == LOCAL_DEVICE {
            return match (asker.tool, asker.from_terminal) {
                (false, _) => Allower::Device { id: LOCAL_DEVICE.to_owned(), name: machine_name().to_owned(), confirmed: true },
                (true, true) => Allower::Terminal,
                (true, false) => Allower::Program,
            };
        }
        let config = self.lock().config.account.clone();
        let owner = crate::account::owner(self, &config).map(|owner| owner.account_id);
        let known = devices.iter().find(|device| device.id == asker.device);
        Allower::Device {
            id: asker.device.clone(),
            name: known.map(|device| shown_name(&device.name)).unwrap_or_default(),
            confirmed: known.is_some_and(|device| device.confirmed_for(owner.as_deref())),
        }
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

    /// The device waiting under `id` with the key `endpoint_id` took back
    /// its request (#603), from its waiting connection: it leaves the
    /// Workspace's pending devices at once.
    pub(crate) fn withdraw_pairing(&self, id: &str, endpoint_id: &str) {
        match withdraw(&self.home, id, endpoint_id) {
            Ok(()) => {
                tracing::info!(device = %id, "a device took back its request to be allowed");
                lock(&self.remote.wrong_numbers).remove(id);
                self.refresh_remote();
            }
            Err(RemoteError::NotWaiting(_)) => {}
            Err(error) => tracing::warn!(device = %id, %error, "could not take back a device's request"),
        }
    }

    /// Whether a session was let in with key `endpoint_id` and is still open.
    fn admitted(&self, endpoint_id: &str) -> bool {
        lock(&self.remote.connected).values().flatten().any(|session| session.endpoint_id == endpoint_id)
    }

    /// Closes a device's sessions and its tunnel connections.
    fn close_device(&self, id: &str, endpoint_id: &str) {
        self.close_sessions(id, endpoint_id, true);
    }

    /// Closes a device's sessions, and with `links` its tunnel connections
    /// (by the keys its sessions came with and `endpoint_id`).
    fn close_sessions(&self, id: &str, endpoint_id: &str, links: bool) {
        let sessions = lock(&self.remote.connected).remove(id).unwrap_or_default();
        self.remote.sessions_changed.send_replace(());
        let mut keys: Vec<&str> = sessions.iter().map(|session| session.endpoint_id.as_str()).collect();
        keys.push(endpoint_id);
        keys.retain(|key| links && !key.is_empty());
        keys.sort_unstable();
        keys.dedup();
        for session in &sessions {
            session.closed.notify_one();
        }
        let open = lock(&self.remote.links);
        for key in keys {
            for (_, connection) in open.get(key).into_iter().flatten() {
                connection.close(UNPAIRED.into(), b"this device was unpaired");
            }
        }
    }

    /// Whether this Claktory is signed in to an account, so the person's
    /// devices may find it; the gate follows.
    pub(crate) fn account_open(&self) -> bool {
        let account = self.lock().config.account.clone();
        let owner = crate::account::owner(self, &account).map(|owner| owner.account_id);
        let open = owner.is_some();
        self.remote.account_open.store(open, Ordering::Relaxed);
        self.recheck(owner.as_deref(), &account);
        open
    }

    /// Turns away the open sessions of devices found through the account
    /// whose account isn't this Claktory's now, `owner` (row 713, Fugo
    /// 49686): signed out, removed at the service, switched to another
    /// account or signing in turned off. Each hears what a new connection
    /// would (signed out: sign it in again; another account's: that), after
    /// the answer it's being sent, if any. A device paired with a code, and
    /// this machine's app, stay connected.
    fn recheck(&self, owner: Option<&str>, config: &crate::config::AccountConfig) {
        let turned: Vec<(Arc<tokio::sync::Notify>, Refusal, String)> = lock(&self.remote.connected)
            .values()
            .flatten()
            .filter(|session| !session.account.is_empty() && owner != Some(session.account.as_str()))
            .map(|session| (session.closed.clone(), session.refusal.clone(), session.platform.clone()))
            .collect();
        if turned.is_empty() {
            return;
        }
        let name = self.identity().name;
        for (closed, why, platform) in turned {
            let mut why = lock(&why);
            if why.is_some() {
                continue;
            }
            *why = Some(if owner.is_none() {
                let (account_service, account_email) = self.account.where_to_sign_in(config);
                v1::Error {
                    code: v1::ErrorCode::SignInRequired.into(),
                    message: SIGNED_OUT.to_owned(),
                    account_service,
                    account_email,
                    ..Default::default()
                }
            } else {
                v1::Error {
                    code: v1::ErrorCode::OtherAccount.into(),
                    message: claktory_switched(&platform, &name, &self.account.where_to_sign_in(config).1),
                    other_account_before: true,
                    ..Default::default()
                }
            });
            closed.notify_one();
        }
        tracing::info!(signed_in = owner.is_some(), "closed the connections of devices this Claktory's account no longer lets in");
    }

    /// Re-reads what the Workspace shows of remote access, and sends it if
    /// it changed. Devices unpaired meanwhile (by `factoryd unpair`) are
    /// disconnected. The tunnel's monitor looks again too.
    pub(crate) fn refresh_remote(&self) {
        self.show_remote();
        self.remote.wake.notify_one();
    }

    /// `refresh_remote`, from the tunnel's monitor itself.
    fn show_remote(&self) {
        let now = now_ms();
        let devices = match devices(&self.home) {
            Ok(devices) => {
                let gone: Vec<String> =
                    lock(&self.remote.connected).keys().filter(|id| !devices.iter().any(|device| &device.id == *id)).cloned().collect();
                for id in gone {
                    self.close_device(&id, "");
                }
                devices
            }
            // A file that can't be read for now (a failing disk, friend bug
            // #38) unpairs nobody: what was shown stays, and the tunnel
            // with it.
            Err(error) => {
                tracing::error!(%error, "could not read the paired devices");
                lock(&self.remote.shown).devices.clone()
            }
        };
        let relays = self.lock().config.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default();
        let on = remote_on(&devices, now) || self.account_open();
        let plan = if on { self.relay_plan() } else { Vec::new() };
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
        let local_offer_dismissed = devices.iter().any(|device| device.local && device.notifications_offer_dismissed);
        let changed = {
            let mut shown = lock(&self.remote.shown);
            let now = Shown {
                // This machine's app isn't one of the devices shown.
                devices: devices.into_iter().filter(|device| !device.local).collect(),
                local_offer_dismissed,
                on,
                relays: plan,
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
                name: shown_name(&device.name),
                kind: device.pending_kind,
                number: approval_number(&shown.endpoint_id, &device.pending_endpoint_id, &device.token_sha256),
                requested_ms: device.pending_since.unwrap_or_default(),
                expires_ms: device.code_expires.unwrap_or_default(),
                signed_in: device.pending_signed_in,
                discovered: device.discovered,
            })
            .collect();
        pending.sort_by_key(|device| device.requested_ms);
        // Each Allow until a device dismisses it (#527 S3), oldest first.
        let mut allow_notices: Vec<v1::AllowNotice> = shown
            .devices
            .iter()
            .filter(|device| device.allow_notice && !device.local && !device.endpoint_id.is_empty())
            .map(|device| v1::AllowNotice {
                device_id: device.id.clone(),
                device_name: shown_name(&device.name),
                allowed_by: device.allowed_by.clone(),
                allowed_by_device_id: device.allowed_by_device.clone(),
                allowed_by_name: device.allowed_by_name.clone(),
                allowed_ms: device.allowed_ms.unwrap_or_default(),
            })
            .collect();
        allow_notices.sort_by_key(|notice| notice.allowed_ms);
        let local_notifications_offer_dismissed = shown.local_offer_dismissed;
        // The backend an update would bring, if it would bring one.
        let offered = Some(inner.latest_release.backend.as_str())
            .filter(|offered| !offered.is_empty() && *offered != crate::update::VERSION)
            .unwrap_or_default();
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
                    // Whether it's the owner's: by an Allow a person made, or
                    // the account this Claktory is signed in to.
                    let confirmed = device.confirmed_for(owner.as_ref().map(|owner| owner.account_id.as_str()));
                    let path =
                        if connected { shown.paths.get(&device.id).copied().unwrap_or(v1::DevicePath::None) } else { v1::DevicePath::None };
                    v1::PairedDevice {
                        connected,
                        path: path.into(),
                        // A device that never bound a key and has no code
                        // (paired before the tunnel) shows as expired.
                        code_expires_ms: if device.endpoint_id.is_empty() { device.code_expires.unwrap_or(device.created) } else { 0 },
                        notifications: crate::notify::push::registered(self, &device.id),
                        // As it said since Claktory started, else as kept with its pushes.
                        notification_permission: self
                            .notify
                            .permission(&device.id)
                            .unwrap_or_else(|| crate::notify::push::permission(self, &device.id))
                            .into(),
                        platform: device.app_platform,
                        update_leaves_behind: leaves_behind(&device.app_version, offered),
                        app_version: device.app_version,
                        id: device.id,
                        name: device.name,
                        paired_ms: device.created,
                        last_seen_ms: device.last_seen.unwrap_or_default(),
                        last_address: device.last_address,
                        confirmed,
                        allowed_ms: device.allowed_ms.unwrap_or_default(),
                        allowed_by: device.allowed_by,
                        allowed_by_device_id: device.allowed_by_device,
                        allowed_by_name: device.allowed_by_name,
                        turned_ms: device.turned_ms.unwrap_or_default(),
                        turned_why: device.turned_why,
                        notifications_offer_dismissed: device.notifications_offer_dismissed,
                        ..v1::PairedDevice::default()
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
            allow_notices,
            local_notifications_offer_dismissed,
        }
    }

    /// DismissNotificationsOffer (area 5a item 4): the "Get notified…"
    /// offer, dismissed on `device` (`LOCAL_DEVICE` for this computer's own
    /// app), until it says its notifications are allowed.
    pub async fn dismiss_notifications_offer(&self, device: &str) -> Result<(), RequestError> {
        self.set_notifications_offer(device, true)
    }

    /// SetActive.notifications from `device`: once they're allowed, the
    /// offer dismissed there is cleared, so it shows again only if they're
    /// turned off later and the person hasn't dismissed it since.
    pub(crate) fn notifications_reported(&self, device: &str, permission: v1::NotificationPermission) {
        if permission != v1::NotificationPermission::Allowed {
            return;
        }
        // Most reports change nothing: only a device that dismissed it is
        // written.
        let dismissed = {
            let shown = lock(&self.remote.shown);
            match device {
                LOCAL_DEVICE => shown.local_offer_dismissed,
                _ => shown.devices.iter().any(|known| known.id == device && known.notifications_offer_dismissed),
            }
        };
        if dismissed && let Err(error) = self.set_notifications_offer(device, false) {
            tracing::error!(%device, %error, "could not clear a device's dismissed notifications offer");
        }
    }

    fn set_notifications_offer(&self, device: &str, dismissed: bool) -> Result<(), RequestError> {
        {
            let _changing = devices_lock();
            let mut devices = devices(&self.home).map_err(|error| RequestError::Remote(error.to_string()))?;
            let Some(known) = devices.iter_mut().find(|known| known.id == device) else {
                return Err(RequestError::Remote("This device isn't one of this Claktory's.".to_owned()));
            };
            if known.notifications_offer_dismissed == dismissed {
                return Ok(());
            }
            known.notifications_offer_dismissed = dismissed;
            save_devices(&self.home, &devices).map_err(|error| RequestError::Remote(error.to_string()))?;
        }
        self.refresh_remote();
        Ok(())
    }

    /// DismissAllowNotice: the notice of a device's Allow leaves Needs you
    /// on every device (#527 S3). One already gone is no error.
    pub async fn dismiss_allow_notice(&self, id: &str) -> Result<(), RequestError> {
        {
            let _changing = devices_lock();
            let mut devices = devices(&self.home).map_err(|error| RequestError::Remote(error.to_string()))?;
            let Some(device) = devices.iter_mut().find(|device| device.id == id && device.allow_notice) else { return Ok(()) };
            device.allow_notice = false;
            save_devices(&self.home, &devices).map_err(|error| RequestError::Remote(error.to_string()))?;
        }
        self.refresh_remote();
        Ok(())
    }

    /// How long making a pairing code waits for the tunnel to reach its
    /// relay (5 s); tests make it shorter.
    pub fn set_tunnel_online_wait(&self, wait: Duration) {
        *lock(&self.remote.online_wait) = Some(wait);
    }

    /// Puts relays and an address lookup on this machine in place of
    /// Claktory's relay and n0's lookup, for tests to need no network:
    /// `relays` become the default ones, and `lookup` sets up the remote
    /// endpoint as `presets::N0` would. Applies from the tunnel's next start.
    pub fn set_tunnel_network(&self, relays: Vec<String>, lookup: impl Fn(Builder) -> Builder + Send + Sync + 'static) {
        *lock(&self.remote.network) = Some(Network { relays, public: None, lookup: Arc::new(lookup) });
    }

    /// Puts `relays` in place of n0's public ones (#450), after
    /// `set_tunnel_network`.
    pub fn set_tunnel_public_relays(&self, relays: Vec<String>) {
        if let Some(network) = lock(&self.remote.network).as_mut() {
            network.public = Some(relays);
        }
    }

    /// How long a relay the tunnel no longer uses is kept (60 s); tests make
    /// it shorter.
    pub fn set_tunnel_relay_grace(&self, grace: Duration) {
        *lock(&self.remote.relay_grace) = Some(grace);
    }

    /// The relays the tunnel uses when config.json names none and it's
    /// signed in to claktory.com.
    fn default_relays(&self) -> Vec<String> {
        lock(&self.remote.network).as_ref().map_or_else(default_relays, |network| network.relays.clone())
    }

    /// n0's public relays, or tests' stand-ins.
    fn public_relays(&self) -> Vec<String> {
        lock(&self.remote.network)
            .as_ref()
            .map_or_else(public_relays, |network| network.public.clone().unwrap_or_else(|| network.relays.clone()))
    }

    /// Whether the tunnel uses Claktory's relays (#450): only while it's
    /// signed in to claktory.com and holds a relay token for its tunnel key,
    /// so it never reaches Claktory's relay without one. That comes with
    /// the first check-in after a sign-in; until then, and after a
    /// sign-out, n0's. A Claktory signed in to an account service of the
    /// person's own holds no token Claktory's relay would take. With tests'
    /// relays in place of the internet, signed in to any.
    fn on_claktorys_relays(&self) -> bool {
        let config = self.lock().config.account.clone();
        let Some((_, _, service)) = self.account.signed_in_as(&config) else { return false };
        let claktorys = service == crate::account::DEFAULT_SERVICE || lock(&self.remote.network).is_some();
        let endpoint_id = lock(&self.remote.shown).endpoint_id.clone();
        claktorys && crate::account::relay_token::current(self, &endpoint_id).is_some()
    }

    /// The relays the tunnel uses now: config.json's; else Claktory's
    /// (`default_relays`) while it's signed in to claktory.com, and n0's
    /// public ones while it isn't (#450).
    pub(crate) fn relay_plan(&self) -> Vec<String> {
        let configured = self.lock().config.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default();
        if !configured.is_empty() {
            configured
        } else if self.on_claktorys_relays() {
            self.default_relays()
        } else {
            self.public_relays()
        }
    }

    /// Whether `url` is Claktory's relay, or the test relay standing for it
    /// (the first of `set_tunnel_network`'s).
    fn is_claktorys(&self, url: &RelayUrl) -> bool {
        match lock(&self.remote.network).as_ref() {
            Some(network) => network.relays.first().is_some_and(|first| first.parse::<RelayUrl>().ok().as_ref() == Some(url)),
            None => is_claktorys_relay(url),
        }
    }

    /// `relay_plan` as iroh takes it: Claktory's relay with this
    /// Claktory's relay token for the tunnel key `endpoint_id`, if it has
    /// one, and every other relay without.
    fn relay_configs(&self, endpoint_id: &str) -> Result<Vec<RelayConfig>, iroh::RelayUrlParseError> {
        let token = crate::account::relay_token::current(self, endpoint_id);
        self.relay_plan()
            .iter()
            .map(|relay| {
                let url = relay.parse::<RelayUrl>()?;
                let ours = self.is_claktorys(&url);
                let config = RelayConfig::from(url);
                Ok(match &token {
                    Some(token) if ours => config.with_auth_token(token.clone()),
                    _ => config,
                })
            })
            .collect()
    }

    /// Pairs a new device, from the app: remote access turns on, and the
    /// code names the relay the tunnel reaches and the addresses it can be
    /// dialled at.
    pub async fn create_pairing(self: &Arc<Self>, name: &str) -> Result<v1::PairingCode, RequestError> {
        let (home, name) = (self.home.clone(), name.to_owned());
        let paired = tokio::task::spawn_blocking(move || pair(&home, &name)).await.map_err(std::io::Error::other)?;
        let (token, device) = paired.map_err(|error| RequestError::Remote(error.to_string()))?;
        let code = self.code_with(&token).await?;
        Ok(v1::PairingCode { link: pairing_link(&code), code, device_id: device.id })
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
    /// too; the default ones while it has reached none, so a code made
    /// before it's online still names one) and its addresses but
    /// 127.0.0.1's.
    pub(crate) fn reached_at(&self, endpoint: &Endpoint) -> (Vec<String>, Vec<SocketAddr>) {
        let addr = endpoint.addr();
        let mut relays: Vec<String> = addr.relay_urls().map(ToString::to_string).collect();
        let configured = self.lock().config.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default();
        let using = match (configured.is_empty(), relays.is_empty()) {
            (false, _) => configured,
            (true, true) => self.relay_plan(),
            (true, false) => Vec::new(),
        };
        for relay in using {
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
        self.forget_devices(std::slice::from_ref(&removed), true).await;
        Ok(())
    }

    /// What goes with devices taken out of the devices file (unpaired, or
    /// replaced by the same device paired again): their sessions close (and
    /// with `links`, their tunnel connections), and their notification keys,
    /// reading places, zoom and file cache go.
    pub(crate) async fn forget_devices(&self, removed: &[Device], links: bool) {
        if removed.is_empty() {
            return;
        }
        for removed in removed {
            self.close_sessions(&removed.id, &removed.endpoint_id, links);
            self.cancel_device_flows(&removed.id);
            // And its notification key.
            crate::notify::push::forget(self, &removed.id);
            // And where it was reading.
            if let Err(error) = crate::presentation::forget_device(self, &removed.id) {
                tracing::warn!(%error, device = %removed.id, "could not forget an unpaired device's reading places");
            }
        }
        self.refresh_remote();
        // And focus mode (#514).
        crate::focus_mode::forget(self, &removed.iter().map(|removed| removed.id.clone()).collect::<Vec<_>>());
        // Their zoom and file cache go with them.
        let kept = {
            let display = &self.lock().config.display;
            removed.iter().any(|removed| display.zoom.contains_key(&removed.id) || display.file_cache.contains_key(&removed.id))
        };
        if kept
            && let Err(error) = self
                .edit_config(|config| {
                    for removed in removed {
                        config.display.zoom.remove(&removed.id);
                        config.display.file_cache.remove(&removed.id);
                    }
                    Ok(())
                })
                .await
        {
            tracing::warn!(%error, "could not forget unpaired devices' zoom and file cache");
        }
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
    let Some(mut woken) = factory.upgrade().map(|factory| factory.loops_woken()) else { return };
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
        strong.show_remote();
        let nap = next_look(&strong);
        let wake = strong.remote.wake.clone();
        drop(strong);
        tokio::select! {
            () = tokio::time::sleep(nap) => {}
            () = wake.notified() => {}
            changed = woken.changed() => if changed.is_err() { return },
        }
    }
}

/// How long the monitor waits before it looks again: a second while an app
/// is connected (it shows the tunnel's state live) or a device waits to be
/// allowed; otherwise until the next pairing code or pipe invitation runs
/// out, at most `IDLE_CHECK`, woken sooner by a change (#454).
fn next_look(factory: &Factory) -> Duration {
    if factory.clients() > 0 || factory.remote.waiting.load(Ordering::Relaxed) > 0 {
        return CHECK;
    }
    let now = now_ms();
    let codes = devices(&factory.home).unwrap_or_default().into_iter().filter_map(|device| device.code_expires);
    let invite = factory.remote.pipes.invite_until();
    match codes.chain(std::iter::once(invite)).filter(|at| *at > now).min() {
        // Just past it, so it has run out.
        Some(at) => Duration::from_millis(u64::try_from(at - now).unwrap_or(0) + 1).min(IDLE_CHECK),
        None => IDLE_CHECK,
    }
}

/// Starts the tunnel's endpoints, or changes them when they should change:
/// the loopback one once, the remote one as remote access and its relays
/// say. Boxed: a client the tunnel serves can pair a device, which comes
/// back here.
fn sync_tunnel(factory: &Arc<Factory>) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
    Box::pin(async move {
        // A file that can't be read for now (a failing disk, friend bug
        // #38) turns nothing off: the devices last shown stand for it.
        let devices = devices(&factory.home).unwrap_or_else(|error| {
            tracing::error!(%error, "could not read the paired devices");
            lock(&factory.remote.shown).devices.clone()
        });
        let relays = factory.lock().config.remote.as_ref().map(|remote| remote.relays.clone()).unwrap_or_default();
        let waiting = factory.remote.waiting.load(Ordering::Relaxed) > 0;
        let pipes = factory.remote.pipes.wants_tunnel(now_ms()) || factory.dm_link.wants_tunnel();
        let open = factory.account_open();
        let wanted = Wanted { on: remote_on(&devices, now_ms()) || waiting || pipes || open, relays };
        let mut tunnel = factory.remote.tunnel.lock().await;
        let remote_as_wanted = match &tunnel.remote {
            Some((relays, ..)) => wanted.on && *relays == wanted.relays,
            None => !wanted.on,
        };
        if tunnel.local.is_some() && remote_as_wanted {
            follow_relays(factory, &mut tunnel).await;
            return;
        }
        if tunnel.failed.as_ref() == Some(&wanted) {
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
                let network = lock(&factory.remote.network).clone();
                let endpoint_id = lock(&factory.remote.shown).endpoint_id.clone();
                let started = match factory.relay_configs(&endpoint_id) {
                    Ok(relays) => {
                        let map = relays.iter().cloned().collect::<RelayMap>();
                        let pipes = &factory.remote.pipes.admits;
                        start_remote(
                            &factory.home,
                            &wanted,
                            map,
                            network.as_ref(),
                            pipes,
                            &factory.dm_link.gate,
                            &factory.remote.account_open,
                        )
                        .await
                        .map(|endpoint| (endpoint, relays))
                    }
                    Err(error) => Err(RemoteError::Tunnel(error.to_string())),
                };
                match started {
                    Ok((endpoint, relays)) => {
                        tracing::info!(endpoint = %endpoint.id(), "remote access started");
                        let accept = tokio::spawn(accept(endpoint.clone(), Arc::downgrade(factory)));
                        *lock(&factory.remote.endpoint) = Some(endpoint.clone());
                        tunnel.remote = Some((wanted.relays.clone(), endpoint, accept));
                        tunnel.relays = relays;
                        tunnel.retiring.clear();
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

/// Keeps the remote endpoint's relays as `relay_plan` says, while it runs
/// (#450): a sign-in or sign-out moves it between Claktory's relays and
/// n0's, and a new relay token goes on Claktory's. New relays are added at
/// once and the Workspace names them (`refresh_remote`); relays no longer
/// used are kept for `RELAY_GRACE`, so devices that follow it there still
/// reach it meanwhile. Never a restart: connections stay up.
async fn follow_relays(factory: &Arc<Factory>, tunnel: &mut Tunnel) {
    let Some((_, endpoint, _)) = &tunnel.remote else { return };
    let endpoint_id = lock(&factory.remote.shown).endpoint_id.clone();
    let wanted = match factory.relay_configs(&endpoint_id) {
        Ok(wanted) => wanted,
        Err(error) => {
            tracing::warn!(%error, "a relay that isn't a URL");
            return;
        }
    };
    for config in &wanted {
        if !tunnel.relays.contains(config) {
            // Its URL only: never the token.
            tracing::info!(relay = %config.url, token = config.auth_token.is_some(), "the tunnel uses a relay");
            endpoint.insert_relay(config.url.clone(), Arc::new(config.clone())).await;
            tunnel.relays.retain(|kept| kept.url != config.url);
            tunnel.relays.push(config.clone());
        }
    }
    let grace = lock(&factory.remote.relay_grace).unwrap_or(RELAY_GRACE);
    let now = Instant::now();
    tunnel.retiring.retain(|(url, _)| !wanted.iter().any(|config| config.url == *url));
    for kept in &tunnel.relays {
        let unused = !wanted.iter().any(|config| config.url == kept.url);
        if unused && !tunnel.retiring.iter().any(|(url, _)| *url == kept.url) {
            tunnel.retiring.push((kept.url.clone(), now + grace));
        }
    }
    let due: Vec<RelayUrl> = tunnel.retiring.iter().filter(|(_, at)| *at <= now).map(|(url, _)| url.clone()).collect();
    for url in due {
        tracing::info!(relay = %url, "the tunnel leaves a relay");
        endpoint.remove_relay(&url).await;
        tunnel.relays.retain(|kept| kept.url != url);
        tunnel.retiring.retain(|(retiring, _)| *retiring != url);
    }
}

/// Signed in or out, or a new relay token (#450): what the Workspace shows
/// now, and the tunnel follows within a second (`follow_relays`).
pub(crate) fn relay_access_changed(factory: &Arc<Factory>) {
    factory.refresh_remote();
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
    let dms = factory.dm_link.gate.clone();
    // Local connections are this machine's whoever is signed in.
    let closed = Arc::new(AtomicBool::new(false));
    let endpoint =
        bind(&home, key, &Wanted { on: false, relays: Vec::new() }, RelayMap::empty(), None, local_port(&home), &pipes, &dms, &closed)
            .await?;
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
    relays: RelayMap,
    network: Option<&Network>,
    pipes: &Arc<crate::pipe_link::Admits>,
    dms: &Arc<crate::dm_link::Admits>,
    open: &Arc<AtomicBool>,
) -> Result<Endpoint, RemoteError> {
    let key = {
        let home = home.clone();
        tokio::task::spawn_blocking(move || secret_key(&home)).await.map_err(std::io::Error::other)??
    };
    let path = dir(home).join("remote-port");
    let port = std::fs::read_to_string(&path).ok().and_then(|text| text.trim().parse().ok()).unwrap_or(0);
    let endpoint = bind(home, key, wanted, relays, network, port, pipes, dms, open).await?;
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
/// With it: every address, the relays (Claktory's own, or config.json's),
/// and n0's address lookup either way: where this Claktory can be reached
/// (its key, home relay and addresses, never data) stays published, so
/// every already-paired device follows a relay change without re-pairing,
/// those whose codes name n0's relays, the default before, too. `network`
/// puts tests' relays and lookup in place of those. Never a port mapping
/// (no portmapper).
// The gate's three lists come apart: pipes, direct messages and the
// account's devices.
#[allow(clippy::too_many_arguments)]
async fn bind(
    home: &Home,
    key: SecretKey,
    wanted: &Wanted,
    relays: RelayMap,
    network: Option<&Network>,
    port: u16,
    pipes: &Arc<crate::pipe_link::Admits>,
    dms: &Arc<crate::dm_link::Admits>,
    open: &Arc<AtomicBool>,
) -> Result<Endpoint, RemoteError> {
    let failed = |error: &dyn std::fmt::Display| RemoteError::Tunnel(error.to_string());
    let builder = |port: u16| -> Result<Builder, RemoteError> {
        let builder = match (wanted.on, network) {
            (false, _) => Endpoint::builder(presets::Minimal),
            (true, None) => Endpoint::builder(presets::N0),
            (true, Some(network)) => (network.lookup)(Endpoint::builder(presets::Minimal)),
        };
        let relay_mode = if wanted.on { RelayMode::Custom(relays.clone()) } else { RelayMode::Disabled };
        let ip: IpAddr = if wanted.on { Ipv4Addr::UNSPECIFIED.into() } else { Ipv4Addr::LOCALHOST.into() };
        let mut builder = builder
            .secret_key(key.clone())
            .alpns(vec![
                ALPN_COMPRESSED.to_vec(),
                ALPN.to_vec(),
                ALPN_FORWARD.to_vec(),
                crate::pipe_link::ALPN_PIPE.to_vec(),
                crate::dm_link::ALPN_DM.to_vec(),
            ])
            .hooks(Gate { home: home.clone(), pipes: pipes.clone(), dms: dms.clone(), open: open.clone() })
            .relay_mode(relay_mode)
            .transport_config(transport())
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

/// iroh's QUIC settings with BBR3 in place of Cubic for what this
/// Claktory sends its devices (row 129). On a slow mobile link (Sale's
/// emulator at 2000 kbit/s) Cubic's bursts overran the link's shaper and
/// it declared lost, and sent again, as much as the file: 14.6 MB on the
/// wire for 6.7 MB of video, most of the "lost" bytes having arrived.
/// BBR3 paces to the bandwidth it measures: 7.5 MB, as fast, and as fast
/// on Wi-Fi; and where a queue is deep it keeps the round trip near the
/// link's own (about 0.1 s where Cubic filled it to 3 s), so a message
/// isn't stuck behind a video. The rest stays iroh's.
///
/// The connection's own keep-alive is [ADMITTED_KEEP_ALIVE]: every path has
/// one of its own (iroh's 5 s until its device is let in, see
/// [pace_admitted]), which is what checks the link, and the connection's
/// would only ping on top of a let-in device's slower ones.
fn transport() -> iroh::endpoint::QuicTransportConfig {
    iroh::endpoint::QuicTransportConfig::builder()
        .congestion_controller_factory(Arc::new(noq_proto::congestion::Bbr3Config::default()))
        .keep_alive_interval(ADMITTED_KEEP_ALIVE)
        .build()
}

/// How often this Claktory checks each path of a device connection whose key
/// a session was let in with (Polpo's idle-keepalive.md, Matt's call). The
/// device checks its own paths every 2 to 5 s while it's in use and every 25
/// s once it's left untouched (app/rust's tunnel.rs), and any packet either
/// way resets both ends' keep-alives, so a check of this Claktory's own every
/// 5 s would keep an idle phone's radio from ever resting.
pub const ADMITTED_KEEP_ALIVE: Duration = Duration::from_secs(25);

/// How long a let-in device's path may stay silent before this Claktory
/// abandons it: two of [ADMITTED_KEEP_ALIVE], and the most it ever gives,
/// whatever the device does (Fugo 51818). The connection as a whole still
/// ends after 30 s with nothing on any path (QUIC's idle timeout), and a
/// device that's unpaired, removed or turned away is closed at once, never
/// left to time out.
pub const ADMITTED_PATH_TIMEOUT: Duration = Duration::from_secs(50);

/// iroh's own keep-alive and idle timeouts (vendor/iroh's `HEARTBEAT_INTERVAL`,
/// `PATH_MAX_IDLE_TIMEOUT`, `RELAY_PATH_MAX_IDLE_TIMEOUT`), which a connection
/// keeps until its key is let in, and gets back once no session is.
const PATH_KEEP_ALIVE: Duration = Duration::from_secs(5);
const PATH_TIMEOUT: Duration = Duration::from_secs(15);
const RELAY_PATH_TIMEOUT: Duration = Duration::from_secs(30);

/// A path's keep-alive and idle timeout: slower for a device let in.
fn path_timers(admitted: bool, relay: bool) -> (Duration, Duration) {
    match (admitted, relay) {
        (true, _) => (ADMITTED_KEEP_ALIVE, ADMITTED_PATH_TIMEOUT),
        (false, true) => (PATH_KEEP_ALIVE, RELAY_PATH_TIMEOUT),
        (false, false) => (PATH_KEEP_ALIVE, PATH_TIMEOUT),
    }
}

/// Paces a device connection's keep-alives while it lives: iroh's own until a
/// session is let in with its key (Fugo 51818: a connection still waiting to
/// be allowed, or turned away, can't hold a path open any longer than
/// before), [ADMITTED_KEEP_ALIVE] and [ADMITTED_PATH_TIMEOUT] while one is,
/// and iroh's again once none is. Paths that open meanwhile follow.
async fn pace_admitted(connection: Connection, factory: Weak<Factory>, endpoint_id: String) {
    let Some(mut changed) = factory.upgrade().map(|strong| strong.remote.sessions_changed.subscribe()) else { return };
    let mut paths = std::pin::pin!(connection.paths_stream());
    let mut given: HashMap<iroh::endpoint::PathId, bool> = HashMap::new();
    loop {
        let Some(strong) = factory.upgrade() else { return };
        let admitted = strong.admitted(&endpoint_id);
        drop(strong);
        let open = connection.paths();
        given.retain(|id, _| open.get(*id).is_some());
        for path in &open {
            // A path shows up with iroh's timers already set (vendor/iroh's
            // `register_and_configure_path`).
            let slow = given.entry(path.id()).or_insert(false);
            if *slow == admitted {
                continue;
            }
            let (keep_alive, timeout) = path_timers(admitted, path.is_relay());
            if path.set_keep_alive_interval(Some(keep_alive)) && path.set_max_idle_timeout(Some(timeout)) {
                *slow = admitted;
            }
        }
        drop(open);
        tokio::select! {
            _ = connection.closed() => return,
            moved = changed.changed() => if moved.is_err() { return },
            next = futures_util::StreamExt::next(&mut paths) => if next.is_none() { return },
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
    tokio::spawn(log_stats(connection.clone(), endpoint_id.clone()));
    if connection.alpn() == crate::pipe_link::ALPN_PIPE {
        crate::pipe_link::serve(connection, factory.clone()).await;
        forget_link(&factory, &endpoint_id, number);
        return;
    }
    if connection.alpn() == crate::dm_link::ALPN_DM {
        crate::dm_link::serve(connection, factory.clone()).await;
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
    tokio::spawn(pace_admitted(connection.clone(), factory.clone(), endpoint_id.clone()));
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

/// How often [log_stats] looks at a connection, and how much it must have
/// sent since its last line to say so again.
const STATS_EVERY: Duration = Duration::from_secs(10);
const STATS_AFTER_BYTES: u64 = 256 * 1024;

/// What a device's connection costs on the wire, in the log (row 129: a
/// video sent twice over on a slow link): QUIC's bytes sent and declared
/// lost (each resent), and per open path what went on it, its congestion
/// events and those found spurious (data only late, resent anyway), its
/// window and round trip. With the log at debug (`RUST_LOG=factoryd=debug`):
/// every [STATS_EVERY] while it sends, and once when it ends.
async fn log_stats(connection: Connection, endpoint: String) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }
    let mut said = 0;
    loop {
        tokio::select! {
            _ = connection.closed() => break,
            () = tokio::time::sleep(STATS_EVERY) => {
                let sent = connection.stats().udp_tx.bytes;
                if sent.saturating_sub(said) >= STATS_AFTER_BYTES {
                    said = sent;
                    stats_line(&connection, &endpoint, "now");
                }
            }
        }
    }
    stats_line(&connection, &endpoint, "at its end");
}

fn stats_line(connection: &Connection, endpoint: &str, when: &str) {
    let stats = connection.stats();
    let paths: Vec<String> = connection
        .paths()
        .iter()
        .map(|path| {
            let at = match path.remote_addr() {
                TransportAddr::Ip(address) => address.to_string(),
                TransportAddr::Relay(url) => format!("relay {url}"),
                other => format!("{other:?}"),
            };
            let path_stats = path.stats();
            format!(
                "{at}{} sent {} B in {} packets, lost {} B in {}, congestion events {} ({} spurious), cwnd {} B, rtt {} ms",
                if path.is_selected() { " (selected)" } else { "" },
                path_stats.sent_bytes,
                path_stats.sent_packets,
                path_stats.lost_bytes,
                path_stats.lost_packets,
                path_stats.congestion_events,
                path_stats.spurious_congestion_events,
                path_stats.cwnd,
                path_stats.rtt.as_millis(),
            )
        })
        .collect();
    tracing::debug!(
        %endpoint,
        alpn = %String::from_utf8_lossy(connection.alpn()),
        udp_sent = stats.udp_tx.bytes,
        udp_received = stats.udp_rx.bytes,
        quic_sent = stats.sent_bytes,
        lost = stats.lost_bytes,
        lost_packets = stats.lost_packets,
        stream_frames_sent = stats.frame_tx.stream,
        paths = %paths.join("; "),
        "a device's connection on the wire, {when}"
    );
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

    /// Row 129: what this Claktory sends is paced by BBR3, and what a
    /// connection cost on the wire is in the log once it ends, its path's
    /// bytes and losses with it.
    #[tokio::test]
    async fn a_connection_is_paced_by_bbr3_and_says_in_the_log_what_it_sent_and_lost() {
        #[derive(Clone, Default)]
        struct Captured(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Captured {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                lock(&self.0).extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber =
            tracing_subscriber::fmt().with_ansi(false).with_max_level(tracing::Level::DEBUG).with_writer(move || writer.clone()).finish();
        let _logging = tracing::subscriber::set_default(subscriber);

        let alpn = b"row129".to_vec();
        let server = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .transport_config(transport())
            .alpns(vec![alpn.clone()])
            .bind()
            .await
            .unwrap();
        let client = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).bind().await.unwrap();
        let accepted = tokio::spawn({
            let server = server.clone();
            async move { server.accept().await.unwrap().await.unwrap() }
        });
        let dialled = client.connect(server.addr(), &alpn).await.unwrap();
        let connection = accepted.await.unwrap();
        let path = connection.paths().iter().next().unwrap().id();
        let controller = connection.congestion_state(path).unwrap().into_any();
        assert!(controller.downcast::<noq_proto::congestion::Bbr3>().is_ok(), "BBR3, not iroh's Cubic");
        let logged = tokio::spawn(log_stats(connection.clone(), "device".into()));
        let mut send = connection.open_uni().await.unwrap();
        send.write_all(&vec![7u8; 1 << 20]).await.unwrap();
        send.finish().unwrap();
        let mut receive = dialled.accept_uni().await.unwrap();
        assert_eq!(receive.read_to_end(2 << 20).await.unwrap().len(), 1 << 20);
        connection.close(0u32.into(), b"done");
        logged.await.unwrap();

        let log = String::from_utf8(lock(&captured.0).clone()).unwrap();
        let line = log.lines().find(|line| line.contains("on the wire, at its end")).unwrap_or_else(|| panic!("{log}"));
        let sent: u64 = line.split("udp_sent=").nth(1).and_then(|rest| rest.split(' ').next()).unwrap().parse().unwrap();
        assert!(sent >= 1 << 20, "{line}");
        assert!(line.contains("lost=") && line.contains("spurious") && line.contains("alpn=row129"), "{line}");
        client.close().await;
        server.close().await;
    }

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
    fn an_update_leaves_behind_only_an_app_on_another_line() {
        assert_eq!(version_line("v0.1.19"), Some((0, 1)));
        assert_eq!(version_line("v0.1.12-dev.3"), Some((0, 1)));
        assert_eq!(version_line("v12.30.0"), Some((12, 30)));
        for unnumbered in ["dev", "2026.11.19-1958", "v0.1", "v0.1.x", "v0.1.+2", "0.1.2", "v0.1.2.3", ""] {
            assert_eq!(version_line(unnumbered), None, "{unnumbered}");
        }
        assert!(leaves_behind("v0.1.19", "v0.2.0"));
        assert!(leaves_behind("v0.2.0", "v0.1.20"), "an app newer than the backend can't either");
        assert!(!leaves_behind("v0.1.19", "v0.1.25"));
        assert!(!leaves_behind("", "v0.2.0"), "an app that never said");
        assert!(!leaves_behind("v0.1.19", ""), "nothing on offer");
        assert!(!leaves_behind("dev", "v0.2.0"));
    }

    #[test]
    fn a_pairing_link_carries_the_code_after_the_hash() {
        let claktory = Identity { name: "Lee's Mac #2".into(), color: "teal".into() };
        let code = pairing_code("ab", "id", &["https://relay.example/".into()], &["127.0.0.1:7".parse().unwrap()], &claktory);
        let link = link_of(&code);
        assert_eq!(link, format!("https://claktory.com/pair#{}", code.strip_prefix("factory://?").unwrap()));
        assert_eq!(link.matches('#').count(), 1, "a # in a value stays encoded: {link}");
        assert!(!link.contains('?'), "nothing before the fragment for the server: {link}");
        assert_eq!(link_of("claktory://add?code=x"), "");
        assert_eq!(link_of(""), "");
        assert_eq!(pairing_link(&code), if PAIR_LINKS { link } else { String::new() }, "only once it's on");
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

    fn knock<'a>(token: &'a str, assertion: &'a str) -> Knock<'a> {
        Knock { token, assertion, kind: v1::DeviceKind::Phone, name: "Matt's  iPhone", platform: "", app_version: "" }
    }

    fn tunnel(key: &str) -> Peer {
        Peer { endpoint_id: key.to_owned(), loopback: false, address: None }
    }

    fn owner(signed_in_ms: i64) -> Owner {
        Owner { account_id: "acct".to_owned(), signed_in_ms, name: "Mac Studio".to_owned(), email: "m…@example.com".to_owned() }
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
        let Admission::Connected(index) =
            admit(&mut devices, &knock("", "A1"), &tunnel("k1"), Some(&owner(now - 29 * MINUTE)), ours(), now)
        else {
            panic!("in at once")
        };
        let first = &devices[index];
        assert_eq!(
            (first.name.as_str(), first.endpoint_id.as_str(), first.confirmed_as.as_str(), first.allowed_by.as_str(), first.allowed_ms),
            ("Matt's iPhone", "k1", "acct", ALLOWED_BY_FIRST_DEVICE, Some(now))
        );
        assert!(!first.allow_notice, "the account let it in: no Allow to tell of");
        assert!(first.discovered && first.code_expires.is_none());
        let again = admit(&mut devices, &knock("", "A1"), &tunnel("k1"), Some(&owner(now - 29 * MINUTE)), ours(), now + 1);
        assert!(matches!(again, Admission::Connected(0)), "back with a fresh assertion");
        // Its key alone is never enough: anything that borrows the device's
        // tunnel shares its key.
        let alone = admit(&mut devices, &knock("", ""), &tunnel("k1"), Some(&owner(now - 29 * MINUTE)), None, now + 1);
        assert!(matches!(alone, Admission::SignInKnown(_)), "not by its key alone");
        let expired = Some(Err(refusal(crate::account::AssertionRefusal::Expired)));
        let stale = admit(&mut devices, &knock("", "A1"), &tunnel("k1"), Some(&owner(now - 29 * MINUTE)), expired, now + 1);
        assert!(
            matches!(stale, Admission::Turned(v1::ErrorCode::SignInRequired, reason) if reason.contains("ran out")),
            "an assertion that ran out asks for a fresh sign-in"
        );

        // Not the first: it waits to be allowed, its digits from the assertion.
        let Admission::Pending(index) = admit(&mut devices, &knock("", "A2"), &tunnel("k2"), Some(&owner(now - 29 * MINUTE)), ours(), now)
        else {
            panic!("waits")
        };
        let second = devices[index].clone();
        assert!(second.discovered && second.pending_signed_in && second.endpoint_id.is_empty());
        assert_eq!((second.pending_endpoint_id.as_str(), second.token_sha256.clone()), ("k2", token_hash("A2")));
        assert!(
            matches!(admit(&mut devices, &knock("", "A2"), &tunnel("k2"), None, ours(), now), Admission::Pending(i) if i == index),
            "still waiting"
        );
        assert!(
            matches!(
                admit(&mut devices, &knock("", ""), &tunnel("k2"), None, None, now),
                Admission::SignInKnown(_)
            ),
            "waiting, but never by its key alone"
        );
        // Its assertion is no token: nobody gets in with it.
        assert!(matches!(admit(&mut devices, &knock("A2", ""), &tunnel("k3"), None, None, now), Admission::Unknown));

        // An old sign-in: even the first device waits.
        let mut devices = Vec::new();
        assert!(matches!(
            admit(&mut devices, &knock("", "A"), &tunnel("k1"), Some(&owner(now - 31 * MINUTE)), ours(), now),
            Admission::Pending(0)
        ));
        let mut devices = Vec::new();
        assert!(
            matches!(admit(&mut devices, &knock("", "A"), &tunnel("k1"), Some(&owner(0)), ours(), now), Admission::Pending(0)),
            "signed in before it was kept"
        );
    }

    #[test]
    fn a_device_without_a_good_assertion_is_told_what_to_do() {
        let mut devices = Vec::new();
        let now = 1_000;
        let turned = admit(&mut devices, &knock("", ""), &tunnel("k"), Some(&owner(now)), None, now);
        assert!(matches!(turned, Admission::Turned(v1::ErrorCode::SignInRequired, _)));
        let refused = Some(Err(refusal(crate::account::AssertionRefusal::OtherAccount)));
        assert!(
            matches!(admit(&mut devices, &knock("", "A"), &tunnel("k"), Some(&owner(now)), refused, now), Admission::Turned(v1::ErrorCode::OtherAccount, reason) if reason.contains("open a pipe"))
        );
        assert!(
            matches!(admit(&mut devices, &knock("", ""), &tunnel("k"), None, None, now), Admission::Unknown),
            "not signed in: as before"
        );
        assert!(devices.is_empty());
    }

    #[test]
    fn a_known_account_device_is_told_to_sign_its_claktory_in_again_not_to_pair_again() {
        let now = 1_000;
        let mut devices = vec![Device {
            id: "mac".into(),
            token_sha256: token_hash("A0"),
            endpoint_id: "k".into(),
            discovered: true,
            ..Device::default()
        }];
        // Signed out (after a disk failure, friend bug #38): a new code
        // would pair it twice.
        let signed_out = Some(Err(refusal(crate::account::AssertionRefusal::NotSignedIn)));
        let turned = admit(&mut devices, &knock("", "A1"), &tunnel("k"), None, signed_out.clone(), now);
        // Not Unauthorized: that says it was removed, and the app wipes
        // what it hadn't sent (#446).
        assert!(matches!(turned, Admission::SignInKnown(reason) if reason == SIGNED_OUT));
        // A device it doesn't know still hears how to be added, as one
        // not paired (Unauthorized).
        let turned = admit(&mut devices, &knock("", "A1"), &tunnel("other"), None, signed_out, now);
        assert!(matches!(turned, Admission::Turned(v1::ErrorCode::Unauthorized, reason) if reason.contains("with a code")));
        // Signed in, an assertion that ran out is still the device's to renew.
        let expired = Some(Err(refusal(crate::account::AssertionRefusal::Expired)));
        let turned = admit(&mut devices, &knock("", "A1"), &tunnel("k"), Some(&owner(1)), expired, now);
        assert!(matches!(turned, Admission::Turned(v1::ErrorCode::SignInRequired, _)));
    }

    /// A waiting device takes back only its own request, by the key it
    /// waits with (#603): a code's goes back to waiting for its code, one
    /// that came through the account is forgotten.
    #[test]
    fn a_waiting_device_takes_back_only_its_own_request() {
        let root = std::env::temp_dir().join(format!("factoryd-withdraw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(root.clone());
        let (_, coded) = pair(&home, "iPad").expect("a code");
        let now = now_ms();
        let mut saved = devices(&home).expect("devices");
        let at = saved.iter().position(|known| known.id == coded.id).expect("there");
        (saved[at].pending_endpoint_id, saved[at].pending_kind, saved[at].pending_since) = ("k".into(), 2, Some(now));
        saved.push(Device {
            id: "mac".into(),
            name: "Mac".into(),
            discovered: true,
            code_expires: Some(now + 60_000),
            pending_endpoint_id: "m".into(),
            pending_signed_in: true,
            ..Device::default()
        });
        save_devices(&home, &saved).expect("save");

        // Another key, or another device's id, takes back nothing.
        assert!(matches!(withdraw(&home, &coded.id, "m"), Err(RemoteError::NotWaiting(_))));
        assert!(matches!(withdraw(&home, "mac", "k"), Err(RemoteError::NotWaiting(_))));
        assert!(matches!(withdraw(&home, &coded.id, ""), Err(RemoteError::NotWaiting(_))));
        assert_eq!(devices(&home).expect("devices").iter().filter(|known| known.is_pending(now)).count(), 2);

        withdraw(&home, &coded.id, "k").expect("its own");
        let after = devices(&home).expect("devices");
        let back = after.iter().find(|known| known.id == coded.id).expect("kept");
        assert!(!back.is_pending(now) && back.awaiting(now), "its code waits again: {back:?}");
        assert!(back.pending_since.is_none() && back.pending_kind == 0);
        withdraw(&home, "mac", "m").expect("its own");
        assert!(!devices(&home).expect("devices").iter().any(|known| known.id == "mac"), "forgotten");
        assert!(matches!(withdraw(&home, "mac", "m"), Err(RemoteError::NotWaiting(_))), "once");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_device_paired_again_with_its_key_replaces_its_earlier_pairing_once_allowed() {
        let root = std::env::temp_dir().join(format!("factoryd-pair-again-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(root.clone());
        let bound = |name: &str, key: &str| {
            let (_, device) = pair(&home, name).expect("paired");
            let mut saved = devices(&home).expect("devices");
            let at = saved.iter().position(|known| known.id == device.id).expect("there");
            saved[at].pending_endpoint_id = key.into();
            save_devices(&home, &saved).expect("save");
            device.id
        };
        let first = bound("MacBook", "k");
        approve(&home, &first).expect("allowed");
        let other = bound("iPhone", "p");
        approve(&home, &other).expect("allowed");
        // The same key with a new code waits, the earlier pairing kept…
        let again = bound("MacBook", "k");
        let ids = |home: &Home| devices(home).expect("devices").into_iter().map(|device| device.id).collect::<Vec<_>>();
        assert_eq!(ids(&home), [first.clone(), other.clone(), again.clone()]);
        // …and turned away, nothing of it is undone.
        deny(&home, &again).expect("denied");
        assert_eq!(ids(&home), [first.clone(), other.clone()]);
        // Allowed, it's the one pairing of that key.
        let again = bound("MacBook", "k");
        let (allowed, replaced) = settle(&home, &again, Ok(&Allower::Terminal)).expect("allowed");
        assert_eq!(replaced.iter().map(|device| device.id.as_str()).collect::<Vec<_>>(), [first.as_str()]);
        assert_eq!((allowed.id.as_str(), allowed.endpoint_id.as_str()), (again.as_str(), "k"));
        assert_eq!(ids(&home), [other, again]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_code_and_the_account_need_no_allow_and_an_allow_a_person_made_confirms() {
        let now = 1_000_000;
        let mut devices = vec![Device {
            id: "phone".into(),
            name: "Phone".into(),
            token_sha256: token_hash("T"),
            code_expires: Some(now + MINUTE),
            ..Device::default()
        }];
        assert!(matches!(admit(&mut devices, &knock("T", "A"), &tunnel("k"), Some(&owner(1)), ours(), now), Admission::Connected(0)));
        assert_eq!((devices[0].endpoint_id.as_str(), devices[0].confirmed_as.as_str(), devices[0].code_expires), ("k", "acct", None));

        // Allowed by a person who compared its digits: the owner's, with
        // no deadline, signed in or not.
        let paired = |allow_confirmed: bool| Device {
            id: "mac".into(),
            token_sha256: token_hash("M"),
            endpoint_id: "m".into(),
            allowed_by: if allow_confirmed { ALLOWED_BY_TERMINAL } else { ALLOWED_BY_PROGRAM }.into(),
            allow_confirmed,
            confirm_by: Some(now - 1),
            ..Device::default()
        };
        let mut devices = vec![paired(true)];
        assert!(matches!(admit(&mut devices, &knock("M", ""), &tunnel("m"), Some(&owner(1)), None, now), Admission::Connected(0)));
        assert_eq!(devices[0].confirm_by, None, "an old deadline is dropped");
        // Let in by a program: the owner's assertion first, at once.
        let mut devices = vec![paired(false)];
        let turned = admit(&mut devices, &knock("M", ""), &tunnel("m"), Some(&owner(1)), None, now);
        assert!(matches!(turned, Admission::SignInKnown(_)));
        assert!(matches!(admit(&mut devices, &knock("M", "A"), &tunnel("m"), Some(&owner(1)), ours(), now), Admission::Connected(0)));
        assert_eq!(devices[0].confirmed_as, "acct");
        // Not signed in, nothing is asked.
        let mut devices = vec![paired(false)];
        assert!(matches!(admit(&mut devices, &knock("M", ""), &tunnel("m"), None, None, now), Admission::Connected(0)));
    }

    #[test]
    fn its_code_reads_as_the_apps() {
        assert_eq!(fingerprint_of("abcdef1234567890ffff"), "abcd ef12 3456 7890");
        assert_eq!(fingerprint_of("abc"), "abc");
    }

    #[test]
    fn a_device_let_in_before_allows_were_recorded_is_the_owners_by_its_allow() {
        let now = 1_000;
        let mut before = Device { id: "mac".into(), endpoint_id: "m".into(), confirm_by: Some(now + 5), ..Device::default() };
        allowed_before(&mut before, now);
        assert!(before.allow_confirmed && before.allowed_by == ALLOWED_BY_BEFORE && !before.allow_notice);
        // Not one confirmed by its account, found through it, waiting, or
        // let in since with a record.
        for mut other in [
            Device { endpoint_id: "m".into(), confirmed_as: "acct".into(), ..Device::default() },
            Device { endpoint_id: "m".into(), discovered: true, ..Device::default() },
            Device { pending_endpoint_id: "m".into(), code_expires: Some(5), ..Device::default() },
            Device { endpoint_id: "m".into(), allowed_by: ALLOWED_BY_PROGRAM.into(), ..Device::default() },
        ] {
            let was = other.clone();
            allowed_before(&mut other, now);
            assert_eq!(other, was);
        }
        // Past its grace, it was being turned away: it still is, signed in,
        // until it confirms; signed out, it comes in and keeps its deadline.
        let mut devices = vec![Device {
            id: "mac".into(),
            token_sha256: token_hash("M"),
            endpoint_id: "m".into(),
            confirm_by: Some(now - 1),
            ..Device::default()
        }];
        allowed_before(&mut devices[0], now);
        assert!(!devices[0].allow_confirmed && devices[0].allowed_by.is_empty());
        let turned = admit(&mut devices, &knock("M", ""), &tunnel("m"), Some(&owner(1)), None, now);
        assert!(matches!(turned, Admission::SignInKnown(_)));
        assert!(matches!(admit(&mut devices, &knock("M", ""), &tunnel("m"), None, None, now), Admission::Connected(0)));
        assert_eq!(devices[0].confirm_by, Some(now - 1), "kept until it confirms");
        allowed_before(&mut devices[0], now);
        assert!(!devices[0].allow_confirmed);
        assert!(matches!(admit(&mut devices, &knock("M", "A"), &tunnel("m"), Some(&owner(1)), ours(), now), Admission::Connected(0)));
        assert_eq!((devices[0].confirmed_as.as_str(), devices[0].confirm_by), ("acct", None));
    }

    /// What `allowed_before` relies on (Fugo 40892): every way a device is
    /// let in now records how, by `allowed_by` or `confirmed_as`, so a
    /// device with neither is one from before.
    #[test]
    fn every_way_in_is_recorded() {
        let now = 100 * MINUTE;
        let recorded = |device: &Device| !device.allowed_by.is_empty() || !device.confirmed_as.is_empty();
        let waiting = |token: &str, key: &str| Device {
            id: format!("d-{token}"),
            token_sha256: token_hash(token),
            code_expires: Some(now + MINUTE),
            pending_endpoint_id: key.into(),
            ..Device::default()
        };
        // The first device through the account.
        let mut devices = Vec::new();
        assert!(matches!(
            admit(&mut devices, &knock("", "A1"), &tunnel("k1"), Some(&owner(now - MINUTE)), ours(), now),
            Admission::Connected(0)
        ));
        assert!(recorded(&devices[0]));
        // A code and the owner's assertion.
        let mut devices = vec![Device { pending_endpoint_id: String::new(), ..waiting("T", "") }];
        assert!(matches!(admit(&mut devices, &knock("T", "A"), &tunnel("k"), Some(&owner(1)), ours(), now), Admission::Connected(0)));
        assert!(recorded(&devices[0]));
        // An Allow, by anyone.
        let root = std::env::temp_dir().join(format!("factoryd-every-way-in-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(root.clone());
        for (index, allower) in [
            Allower::Terminal,
            Allower::Program,
            Allower::Device { id: "local".into(), name: "Mac".into(), confirmed: true },
            Allower::Device { id: "x".into(), name: "X".into(), confirmed: false },
        ]
        .iter()
        .enumerate()
        {
            let (_, made) = pair(&home, "Device").expect("paired");
            let mut saved = devices_as_saved(&home).expect("devices");
            let at = saved.iter().position(|known| known.id == made.id).expect("there");
            saved[at].pending_endpoint_id = format!("key-{index}");
            save_devices(&home, &saved).expect("save");
            let (allowed, _) = settle(&home, &made.id, Ok(allower)).expect("allowed");
            assert!(recorded(&allowed) && !allowed.endpoint_id.is_empty(), "{allower:?}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_assertion_that_ran_out_is_ignored_only_for_a_device_a_person_allowed() {
        let now = 1_000_000;
        let expired = || Some(Err(refusal(crate::account::AssertionRefusal::Expired)));
        let device = |allow_confirmed: bool| Device {
            id: "mac".into(),
            token_sha256: token_hash("M"),
            endpoint_id: "m".into(),
            allowed_by: ALLOWED_BY_TERMINAL.into(),
            allow_confirmed,
            ..Device::default()
        };
        // Allowed by a person: as if it sent none.
        let mut devices = vec![device(true)];
        assert!(matches!(admit(&mut devices, &knock("M", "A"), &tunnel("m"), Some(&owner(1)), expired(), now), Admission::Connected(0)));
        // Not by its key on another device, nor without its code.
        assert!(matches!(
            admit(&mut devices, &knock("M", "A"), &tunnel("other"), Some(&owner(1)), expired(), now),
            Admission::Turned(v1::ErrorCode::SignInRequired, _)
        ));
        // Let in by a program: it still has to sign in.
        let mut devices = vec![device(false)];
        assert!(matches!(
            admit(&mut devices, &knock("M", "A"), &tunnel("m"), Some(&owner(1)), expired(), now),
            Admission::Turned(v1::ErrorCode::SignInRequired, _)
        ));
        // Anything but Expired still refuses a device a person allowed.
        let mut devices = vec![device(true)];
        for refused in [
            crate::account::AssertionRefusal::OtherAccount,
            crate::account::AssertionRefusal::OtherKey,
            crate::account::AssertionRefusal::Invalid,
        ] {
            let turned = admit(&mut devices, &knock("M", "A"), &tunnel("m"), Some(&owner(1)), Some(Err(refusal(refused))), now);
            assert!(matches!(turned, Admission::Turned(..) | Admission::OtherAccountNow(_)), "{turned:?}");
        }
        // Only Expired reads as SignInRequired.
        for refused in [
            crate::account::AssertionRefusal::OtherAccount,
            crate::account::AssertionRefusal::OtherKey,
            crate::account::AssertionRefusal::Invalid,
            crate::account::AssertionRefusal::NotSignedIn,
        ] {
            assert_ne!(refusal(refused).0, v1::ErrorCode::SignInRequired);
        }
    }

    #[test]
    fn another_accounts_device_is_turned_away_before_and_after_allow_and_a_bad_assertion_is_never_ignored() {
        let now = 1_000_000;
        let other = || Some(Err(refusal(crate::account::AssertionRefusal::OtherAccount)));
        let waiting = Device { id: "new".into(), token_sha256: token_hash("T"), code_expires: Some(now + MINUTE), ..Device::default() };
        let mut devices = vec![waiting.clone()];
        let turned = admit(&mut devices, &knock("T", "A"), &tunnel("k"), Some(&owner(1)), other(), now);
        assert!(matches!(turned, Admission::Turned(v1::ErrorCode::OtherAccount, reason) if reason.contains("open a pipe")));
        assert_eq!(devices, std::slice::from_ref(&waiting), "nothing to allow");
        // Allowed and confirmed, then signed in to another account.
        let mut devices = vec![Device {
            id: "mac".into(),
            token_sha256: token_hash("M"),
            endpoint_id: "m".into(),
            allow_confirmed: true,
            ..Device::default()
        }];
        let turned = admit(&mut devices, &knock("M", "A"), &tunnel("m"), Some(&owner(1)), other(), now);
        assert!(
            matches!(turned, Admission::OtherAccountNow(reason) if reason.contains("now signed in to a different") && reason.contains("Mac Studio"))
        );
        // Noted on it, once a minute however often it tries, and cleared
        // when it next gets in.
        assert_eq!((devices[0].turned_ms, devices[0].turned_why.as_str()), (Some(now), TURNED_OTHER_ACCOUNT));
        admit(&mut devices, &knock("M", "A"), &tunnel("m"), Some(&owner(1)), other(), now + 1);
        assert_eq!(devices[0].turned_ms, Some(now));
        admit(&mut devices, &knock("M", "A"), &tunnel("m"), Some(&owner(1)), other(), now + TURNED_EVERY);
        assert_eq!(devices[0].turned_ms, Some(now + TURNED_EVERY));
        assert!(matches!(
            admit(&mut devices, &knock("M", "A"), &tunnel("m"), Some(&owner(1)), ours(), now + TURNED_EVERY + 1),
            Admission::Connected(0)
        ));
        assert_eq!((devices[0].turned_ms, devices[0].turned_why.as_str()), (None, ""));
        // An assertion that doesn't verify is refused, not taken for none;
        // with a code, never as Unauthorized (#446). (One that only ran out,
        // from a device a person allowed, is the exception: next test.)
        let turned = admit(
            &mut devices,
            &knock("M", "A"),
            &tunnel("m"),
            Some(&owner(1)),
            Some(Err(refusal(crate::account::AssertionRefusal::OtherKey))),
            now,
        );
        assert!(matches!(turned, Admission::Turned(v1::ErrorCode::SignInRequired, _)), "{turned:?}");
        for refused in [crate::account::AssertionRefusal::Expired, crate::account::AssertionRefusal::OtherKey] {
            let mut fresh = vec![waiting.clone()];
            let turned = admit(&mut fresh, &knock("T", "A"), &tunnel("k"), Some(&owner(1)), Some(Err(refusal(refused))), now);
            assert!(matches!(turned, Admission::Turned(v1::ErrorCode::SignInRequired, _)));
        }
        // Signed out, there's no owner to compare with: as before.
        let mut devices = vec![waiting];
        let not_signed_in = Some(Err(refusal(crate::account::AssertionRefusal::NotSignedIn)));
        assert!(matches!(admit(&mut devices, &knock("T", "A"), &tunnel("k"), None, not_signed_in, now), Admission::Pending(0)));
    }

    /// The example the app checks its side against.
    #[test]
    fn a_waiting_devices_name_shows_without_control_characters_and_cut_to_40() {
        assert_eq!(shown_name("Laptop\u{7}\u{1b}[31m"), "Laptop[31m");
        assert_eq!(shown_name("\u{202E}enohP\u{2066}x\u{2069}\u{200B}\u{FEFF}\u{2060}"), "enohPx");
        assert_eq!(shown_name("i\u{061C}Pad\u{2061}\u{206A}"), "iPad", "row 1730: every character that hides or reorders text");
        assert_eq!(own_name(" Matt's\u{202E}  iPhone\u{200B}\n"), "Matt's iPhone", "a device's own name is kept so too");
        let long = "x".repeat(60);
        assert_eq!(shown_name(&long), format!("{}…", "x".repeat(39)));
        assert_eq!(shown_name(&"y".repeat(40)), "y".repeat(40));
    }

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
