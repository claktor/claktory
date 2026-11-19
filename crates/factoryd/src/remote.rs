//! Devices on other machines: a second listener, off unless config.json's
//! `remote` turns it on, that serves the same protocol over TLS to devices
//! paired with `factoryd pair <name>`.
//!
//! - The certificate is Factory's own, made once (`<home>/remote/cert.pem`
//!   and `key.pem`); devices pin its SHA-256, so no CA or domain is needed.
//! - Each paired device has a token, given once in its pairing code; Factory
//!   keeps only the token's SHA-256 (`<home>/remote/devices.json`), read
//!   afresh at every handshake, so `factoryd unpair` applies at once.
//! - A remote device can't make Factory replace an app bundle on this
//!   machine (`InstallUpdate.app_path` is ignored for it).
//!
//! The local listener (127.0.0.1) stays as it is, with no token.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use factory_protocol::v1;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::config::{Config, RemoteConfig};
use crate::factory::{Factory, Inner, RequestError};
use crate::home::Home;
use crate::transcript::now_ms;

/// How often the listener checks config.json's `remote` for a change.
const CHECK: Duration = Duration::from_secs(1);
/// A device that gave a wrong token waits at least this long for the answer
/// to its next try.
const FAILURE_PAUSE: Duration = Duration::from_secs(1);
/// A handshake (TLS and WebSocket) gets this long.
const HANDSHAKE: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("couldn't make the certificate: {0}")]
    Certificate(#[from] rcgen::Error),
    #[error("the certificate or key in {0} can't be read")]
    BadCertificate(PathBuf),
    #[error("TLS: {0}")]
    Tls(#[from] rustls::Error),
    #[error("{0} is broken: {1}")]
    Devices(PathBuf, serde_json::Error),
    #[error("couldn't make a token: {0}")]
    Random(#[from] getrandom::Error),
    #[error(
        "Factory's remote listener is off. Turn it on first: run factoryd remote on, or add \"remote\": {{\"port\": 7443}} to {0} (and \"listen\": \"<address>\" to listen on one address only, such as the machine's Tailscale address)."
    )]
    Off(PathBuf),
    #[error("{0} isn't there yet: start Factory once first")]
    NoConfig(PathBuf),
    #[error("{0} isn't valid: {1}")]
    Config(PathBuf, String),
    #[error("no device named \"{0}\" is paired")]
    NoDevice(String),
}

/// Where Factory keeps what remote devices need.
fn dir(home: &Home) -> PathBuf {
    home.root().join("remote")
}

/// Factory's certificate and its key, made the first time.
struct Certificate {
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

impl Certificate {
    fn load_or_make(home: &Home) -> Result<Certificate, RemoteError> {
        let dir = dir(home);
        let (cert_path, key_path) = (dir.join("cert.pem"), dir.join("key.pem"));
        if !cert_path.exists() || !key_path.exists() {
            let made = rcgen::generate_simple_self_signed(vec!["factory".to_owned()])?;
            std::fs::create_dir_all(&dir)?;
            write_private(&key_path, made.signing_key.serialize_pem().as_bytes())?;
            write_private(&cert_path, made.cert.pem().as_bytes())?;
        }
        let cert = CertificateDer::from_pem_file(&cert_path).map_err(|_| RemoteError::BadCertificate(cert_path))?;
        let key = PrivateKeyDer::from_pem_file(&key_path).map_err(|_| RemoteError::BadCertificate(key_path))?;
        Ok(Certificate { cert, key })
    }

    /// What devices pin: the SHA-256 of the certificate, in hex.
    fn fingerprint(&self) -> String {
        hex(&Sha256::digest(self.cert.as_ref()))
    }

    fn acceptor(self) -> Result<TlsAcceptor, RemoteError> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_single_cert(vec![self.cert], self.key)?;
        Ok(TlsAcceptor::from(Arc::new(config)))
    }
}

/// Written so only this user can read it.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let partial = path.with_extension("partial");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(partial, path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
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
    /// The IP address it last connected from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_address: String,
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

fn save_devices(home: &Home, devices: &[Device]) -> Result<(), RemoteError> {
    std::fs::create_dir_all(dir(home))?;
    let mut json = serde_json::to_string_pretty(devices).map_err(|error| RemoteError::Devices(devices_path(home), error))?;
    json.push('\n');
    write_private(&devices_path(home), json.as_bytes())?;
    Ok(())
}

/// The port remote access takes when it's first turned on.
pub(crate) const DEFAULT_PORT: u16 = 7443;

/// The listener's settings in config.json as they are on disk.
fn remote_config(home: &Home) -> Result<RemoteConfig, RemoteError> {
    let path = home.config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(RemoteError::Off(path)),
        Err(error) => return Err(error.into()),
    };
    let config = Config::parse(&text).map_err(|error| RemoteError::Config(path.clone(), error.to_string()))?;
    config.remote.ok_or(RemoteError::Off(path))
}

/// Turns the remote listener on or off from the command line (`factoryd
/// remote on [PORT]`, `factoryd remote off`), by writing config.json as the
/// app's switch does; a running Factory applies it within a second. Turned
/// on, it keeps the port and address already set unless given a port. Returns
/// the setting as it is now.
pub fn set_remote(home: &Home, on: bool, port: Option<u16>) -> Result<Option<RemoteConfig>, RemoteError> {
    let path = home.config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(RemoteError::NoConfig(path)),
        Err(error) => return Err(error.into()),
    };
    let invalid = |error: &dyn std::fmt::Display| RemoteError::Config(path.clone(), error.to_string());
    let mut config = Config::parse(&text).map_err(|error| invalid(&error))?;
    let current = config.remote.take();
    config.remote = on.then(|| RemoteConfig {
        port: port.unwrap_or(current.as_ref().map_or(DEFAULT_PORT, |current| current.port)),
        listen: current.map(|current| current.listen).unwrap_or_default(),
    });
    config.validate().map_err(|error| invalid(&error))?;
    let text = config.to_json().map_err(|error| invalid(&error))?;
    let partial = path.with_extension("json.partial");
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, &path)?;
    Ok(config.remote)
}

/// Pairs a new device: makes its token and returns its pairing code, which
/// holds the token, so it's shown once and never again:
/// `factory://<host>:<port>?t=<token>&fp=<certificate sha256>&name=<this machine>`.
/// `host` is the address the device will reach this machine by; empty for
/// the best of `addresses`.
pub fn pair(home: &Home, name: &str, host: &str) -> Result<(String, Device), RemoteError> {
    let remote = remote_config(home)?;
    let addr = remote.addr().map_err(|error| RemoteError::Config(home.config_path(), error.to_string()))?;
    let fingerprint = Certificate::load_or_make(home)?.fingerprint();
    let mut token = [0_u8; 32];
    getrandom::fill(&mut token)?;
    let token = hex(&token);
    let mut devices = devices(home)?;
    let name = name.trim();
    let base: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let base = base.trim_matches('-');
    let base = if base.is_empty() { "device" } else { base };
    let mut id = base.to_owned();
    let mut n = 2;
    while devices.iter().any(|device| device.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    let device = Device {
        id,
        name: if name.is_empty() { base.to_owned() } else { name.to_owned() },
        token_sha256: hex(&Sha256::digest(token.as_bytes())),
        created: now_ms(),
        last_seen: None,
        last_address: String::new(),
    };
    devices.push(device.clone());
    save_devices(home, &devices)?;
    let host = match host.trim() {
        "" => addresses(&remote).into_iter().next().unwrap_or_else(|| machine_name().to_owned()),
        host => host.to_owned(),
    };
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host };
    let code = format!("factory://{host}:{}?t={token}&fp={fingerprint}&name={}", addr.port(), encode(machine_name()));
    Ok((code, device))
}

/// Unpairs the device with this id or name; it can't connect any more.
pub fn unpair(home: &Home, device: &str) -> Result<Device, RemoteError> {
    let mut devices = devices(home)?;
    let Some(index) = devices.iter().position(|known| known.id == device || known.name == device) else {
        return Err(RemoteError::NoDevice(device.to_owned()));
    };
    let removed = devices.remove(index);
    save_devices(home, &devices)?;
    Ok(removed)
}

/// This machine's name, as devices show it ("mac-mini").
fn machine_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let name = std::process::Command::new("hostname")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default();
        let name = name.strip_suffix(".local").unwrap_or(&name).to_owned();
        if name.is_empty() { "factory".to_owned() } else { name }
    })
}

/// How other machines may reach this one, best first: the address it listens
/// on only, if it does; else its Tailscale addresses, its name on the local
/// network (`<name>.local`), then its other addresses; on Linux, its other
/// addresses, then its name.
pub(crate) fn addresses(remote: &RemoteConfig) -> Vec<String> {
    if !remote.listen.is_empty() {
        return vec![remote.listen.clone()];
    }
    let ips = interface_ips();
    let tailscale = |ip: &IpAddr| match ip {
        IpAddr::V4(ip) => ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64,
        IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    };
    let other = |ip: &IpAddr| match ip {
        IpAddr::V4(ip) => !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified(),
        IpAddr::V6(_) => false,
    };
    let mut addresses: Vec<String> = ips.iter().filter(|ip| tailscale(ip)).map(IpAddr::to_string).collect();
    let others = ips.iter().filter(|ip| !tailscale(ip) && other(ip)).map(IpAddr::to_string);
    // A Mac answers to <name>.local on its network; elsewhere a bare name
    // seldom resolves from another machine, so it comes after the addresses.
    if cfg!(target_os = "macos") {
        addresses.push(format!("{}.local", machine_name()));
        addresses.extend(others);
    } else {
        addresses.extend(others);
        addresses.push(machine_name().to_owned());
    }
    addresses.dedup();
    addresses
}

/// This machine's IP addresses, from its network interfaces.
fn interface_ips() -> Vec<IpAddr> {
    let mut ips = Vec::new();
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `list` with a linked list that stays valid
    // until freeifaddrs; each entry's address is checked for null and read
    // as the family it says it is.
    unsafe {
        if libc::getifaddrs(&raw mut list) != 0 {
            return ips;
        }
        let mut cursor = list;
        while !cursor.is_null() {
            let addr = (*cursor).ifa_addr;
            if !addr.is_null() {
                match i32::from((*addr).sa_family) {
                    libc::AF_INET => {
                        let addr = &*addr.cast::<libc::sockaddr_in>();
                        ips.push(IpAddr::V4(Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr))));
                    }
                    libc::AF_INET6 => {
                        let addr = &*addr.cast::<libc::sockaddr_in6>();
                        ips.push(IpAddr::V6(Ipv6Addr::from(addr.sin6_addr.s6_addr)));
                    }
                    _ => {}
                }
            }
            cursor = (*cursor).ifa_next;
        }
        libc::freeifaddrs(list);
    }
    ips
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

/// What Factory keeps of remote access while it runs.
#[derive(Default)]
pub(crate) struct State {
    shown: Mutex<Shown>,
    /// The open connections of each device, each with what closes it.
    connected: Mutex<HashMap<String, Vec<Arc<tokio::sync::Notify>>>>,
    /// When each address last gave a wrong token.
    failures: Mutex<HashMap<IpAddr, Instant>>,
}

/// What the Workspace shows of remote access, besides config.json's `remote`.
#[derive(Clone, Default, PartialEq)]
struct Shown {
    devices: Vec<Device>,
    error: String,
    fingerprint: String,
    addresses: Vec<String>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// One of a device's connections; it counts as connected while this lives.
pub(crate) struct Connected {
    pub device: Device,
    /// Notified when the device is unpaired: the connection closes.
    pub closed: Arc<tokio::sync::Notify>,
    factory: Weak<Factory>,
}

impl Drop for Connected {
    fn drop(&mut self) {
        let Some(factory) = self.factory.upgrade() else { return };
        {
            let mut connected = lock(&factory.remote.connected);
            if let Some(connections) = connected.get_mut(&self.device.id) {
                connections.retain(|closed| !Arc::ptr_eq(closed, &self.closed));
                if connections.is_empty() {
                    connected.remove(&self.device.id);
                }
            }
        }
        factory.broadcast_workspace(&factory.lock());
    }
}

impl Factory {
    /// The paired device whose token this is, connected until the returned
    /// `Connected` goes. A wrong token is answered no sooner than a second
    /// after the address's last wrong one.
    pub(crate) async fn authorize(self: &Arc<Self>, token: &str, peer: SocketAddr) -> Option<Connected> {
        let digest = hex(&Sha256::digest(token.as_bytes()));
        let mut found = None;
        match devices(&self.home) {
            Ok(mut devices) => {
                for device in &mut devices {
                    // Compared in full every time, whatever matches.
                    let known = device.token_sha256.as_bytes();
                    let same =
                        known.len() == digest.len() && known.iter().zip(digest.as_bytes()).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0;
                    if same && !token.is_empty() {
                        device.last_seen = Some(now_ms());
                        device.last_address = peer.ip().to_canonical().to_string();
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
        let Some(device) = found else {
            let wait = {
                let now = Instant::now();
                let last = lock(&self.remote.failures).insert(peer.ip(), now);
                last.map(|last| FAILURE_PAUSE.saturating_sub(now.duration_since(last))).unwrap_or_default()
            };
            tracing::warn!(%peer, "refused a device that isn't paired");
            tokio::time::sleep(wait).await;
            return None;
        };
        let closed = Arc::new(tokio::sync::Notify::new());
        lock(&self.remote.connected).entry(device.id.clone()).or_default().push(closed.clone());
        self.refresh_remote();
        Some(Connected { device, closed, factory: Arc::downgrade(self) })
    }

    /// Re-reads what the Workspace shows of remote access, and sends it if
    /// it changed.
    fn refresh_remote(&self) {
        let remote = self.lock().config.remote.clone();
        let devices = devices(&self.home).unwrap_or_else(|error| {
            tracing::error!(%error, "could not read the paired devices");
            Vec::new()
        });
        let changed = {
            let mut shown = lock(&self.remote.shown);
            let mut now = Shown { devices, ..shown.clone() };
            now.addresses = remote.as_ref().map(addresses).unwrap_or_default();
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
    pub(crate) fn remote_view(&self, inner: &Inner) -> v1::RemoteAccess {
        let shown = lock(&self.remote.shown).clone();
        let connected = lock(&self.remote.connected);
        let remote = inner.config.remote.as_ref();
        v1::RemoteAccess {
            on: remote.is_some(),
            port: remote.map_or(0, |remote| u32::from(remote.port)),
            listen: remote.map(|remote| remote.listen.clone()).unwrap_or_default(),
            error: if remote.is_some() { shown.error } else { String::new() },
            fingerprint: shown.fingerprint,
            addresses: if remote.is_some() { shown.addresses } else { Vec::new() },
            devices: shown
                .devices
                .into_iter()
                .map(|device| v1::PairedDevice {
                    connected: connected.contains_key(&device.id),
                    id: device.id,
                    name: device.name,
                    paired_ms: device.created,
                    last_seen_ms: device.last_seen.unwrap_or_default(),
                    last_address: device.last_address,
                })
                .collect(),
            machine: machine_name().to_owned(),
        }
    }

    /// Pairs a new device, from the app.
    pub async fn create_pairing(&self, name: &str, host: &str) -> Result<v1::PairingCode, RequestError> {
        if self.lock().config.remote.is_none() {
            return Err(RequestError::Remote("Remote access is off: turn it on first.".into()));
        }
        let (home, name, host) = (self.home.clone(), name.to_owned(), host.to_owned());
        let paired = tokio::task::spawn_blocking(move || pair(&home, &name, &host)).await.map_err(std::io::Error::other)?;
        let (code, device) = paired.map_err(|error| RequestError::Remote(error.to_string()))?;
        self.refresh_remote();
        Ok(v1::PairingCode { code, device_id: device.id })
    }

    /// The folders in a folder of this machine, for an app on another one
    /// to pick a project's folder by browsing rather than typing its path.
    /// `path` is absolute, or starts at the home folder ("", "~" or "~/…").
    pub async fn list_folders(&self, path: &str) -> Result<v1::FolderList, RequestError> {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || list_folders(&path)).await.map_err(std::io::Error::other)?.map_err(RequestError::Remote)
    }

    /// Unpairs a device, from the app: its connections close.
    pub async fn unpair_device(&self, id: &str) -> Result<(), RequestError> {
        match unpair(&self.home, id) {
            Ok(_) => {}
            Err(RemoteError::NoDevice(id)) => return Err(RequestError::UnknownDevice(id)),
            Err(error) => return Err(RequestError::Remote(error.to_string())),
        }
        for closed in lock(&self.remote.connected).remove(id).unwrap_or_default() {
            closed.notify_one();
        }
        self.refresh_remote();
        Ok(())
    }

    /// Turns remote access on or off, from the app.
    pub async fn set_remote_access(&self, on: bool, port: u32, listen: Option<&str>) -> Result<(), RequestError> {
        let port = u16::try_from(port).map_err(|_| RequestError::Remote(format!("{port} isn't a port")))?;
        self.edit_config(|config| {
            config.remote = on.then(|| {
                let current = config.remote.clone();
                RemoteConfig {
                    port: if port != 0 { port } else { current.as_ref().map_or(DEFAULT_PORT, |current| current.port) },
                    listen: match listen {
                        Some(listen) => listen.trim().to_owned(),
                        None => current.map(|current| current.listen).unwrap_or_default(),
                    },
                }
            });
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

/// Keeps the remote listener as config.json's `remote` says: off, or on its
/// address, started, moved or stopped within a second of a change. Also
/// keeps what the Workspace shows of it current: devices paired or unpaired
/// by `factoryd`, and the machine's addresses.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    let mut serving: Option<(RemoteConfig, tokio::task::JoinHandle<()>)> = None;
    // What failed last, so it isn't tried every second.
    let mut failed: Option<RemoteConfig> = None;
    loop {
        let wanted = match factory.upgrade() {
            Some(factory) => {
                factory.refresh_remote();
                factory.lock().config.remote.clone()
            }
            None => break,
        };
        if failed.is_some() && failed != wanted {
            failed = None;
        }
        if serving.as_ref().map(|(config, _)| config) != wanted.as_ref() && failed.is_none() {
            if let Some((_, task)) = serving.take() {
                task.abort();
                tracing::info!("remote listener stopped");
            }
            let mut error = String::new();
            if let Some(config) = wanted {
                match listen(&factory, &config).await {
                    Ok((task, fingerprint)) => {
                        serving = Some((config, task));
                        if let Some(factory) = factory.upgrade() {
                            factory.set_remote_shown(|shown| shown.fingerprint = fingerprint);
                        }
                    }
                    Err(problem) => {
                        tracing::error!(error = %problem, "could not start the remote listener");
                        error = problem.to_string();
                        failed = Some(config);
                    }
                }
            }
            if let Some(factory) = factory.upgrade() {
                factory.set_remote_shown(|shown| shown.error = error);
            }
        }
        tokio::time::sleep(CHECK).await;
    }
    if let Some((_, task)) = serving {
        task.abort();
    }
}

async fn listen(factory: &Weak<Factory>, config: &RemoteConfig) -> Result<(tokio::task::JoinHandle<()>, String), RemoteError> {
    let Some(strong) = factory.upgrade() else { return Err(std::io::Error::other("Factory is stopping").into()) };
    let addr = config.addr().map_err(|error| RemoteError::Config(strong.home.config_path(), error.to_string()))?;
    let home = strong.home.clone();
    drop(strong);
    let (acceptor, fingerprint) = tokio::task::spawn_blocking(move || {
        let certificate = Certificate::load_or_make(&home)?;
        let fingerprint = certificate.fingerprint();
        Ok::<_, RemoteError>((certificate.acceptor()?, fingerprint))
    })
    .await
    .map_err(|error| std::io::Error::other(error.to_string()))??;
    let listener =
        TcpListener::bind(addr).await.map_err(|error| std::io::Error::new(error.kind(), format!("can't listen on {addr}: {error}")))?;
    tracing::info!(%addr, "remote listener started");
    Ok((tokio::spawn(accept(listener, acceptor, factory.clone())), fingerprint))
}

async fn accept(listener: TcpListener, acceptor: TlsAcceptor, factory: Weak<Factory>) {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                tracing::warn!(%error, "remote accept failed");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Some(factory) = factory.upgrade() else { return };
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            if let Err(error) = stream.set_nodelay(true) {
                tracing::debug!(%peer, %error, "remote connection");
                return;
            }
            let stream = match tokio::time::timeout(HANDSHAKE, acceptor.accept(stream)).await {
                Ok(Ok(stream)) => stream,
                // Port scans and clients that don't speak TLS.
                Ok(Err(error)) => return tracing::debug!(%peer, %error, "not a TLS client"),
                Err(_) => return tracing::debug!(%peer, "TLS handshake timed out"),
            };
            if let Err(error) = crate::connection::serve_stream(stream, factory, Some(peer)).await {
                tracing::warn!(%peer, %error, "remote connection failed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_in_pairing_codes_are_percent_encoded() {
        assert_eq!(encode("Mac mini/2"), "Mac%20mini%2F2");
        assert_eq!(encode("box-1.lan"), "box-1.lan");
    }

    #[test]
    fn a_listen_address_is_the_only_one_offered() {
        let remote = RemoteConfig { port: 7443, listen: "100.101.102.103".into() };
        assert_eq!(addresses(&remote), ["100.101.102.103"]);
        let everywhere = addresses(&RemoteConfig { port: 7443, listen: String::new() });
        assert!(everywhere.iter().any(|address| address.starts_with(machine_name())), "{everywhere:?}");
        assert!(!everywhere.iter().any(|address| address == "127.0.0.1"), "{everywhere:?}");
    }
}
