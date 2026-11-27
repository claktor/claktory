//! Who Claktory's relay forwards for.
//!
//! Every client gets in. A Claktory signed in to a claktory.com account
//! shows a relay token (`Authorization: Bearer`), which the account service
//! signs and binds to its endpoint id. The relay forwards a packet only when
//! its sender or its recipient showed a good one. A signed-in Claktory's
//! devices, phones pairing with it and pipe peers that aren't signed in
//! reach it as before, while two endpoints that aren't Claktory users can't
//! use the relay with each other.
//!
//! In `observe` mode nothing is dropped and what would be is counted, so the
//! switch to `enforce` can wait until every Claktory in use sends tokens.

use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use iroh_base::{EndpointId, PublicKey, Signature};
use iroh_metrics::{Counter, Gauge, MetricsGroup};
use iroh_relay::server::{Access, AccessControl, ClientRequest, ConnectionId};
use serde::Deserialize;
use tracing::{debug, info, warn};

/// What the account service signs before a token's payload, so its key
/// never signs anything that reads as another protocol's message.
pub const DOMAIN: &[u8] = b"claktory-relay-token-v1\0";
/// The longest token looked at.
pub const MAX_TOKEN: usize = 512;
/// The largest key file read.
pub const MAX_KEYS_FILE: u64 = 4096;
/// The most accounts a denylist may name.
pub const MAX_DENIED: usize = 100_000;
/// The largest denylist file read: `MAX_DENIED` entries and room to spare.
const MAX_DENYLIST_FILE: u64 = 8 << 20;
/// The longest a token may live: the account service gives 7 days.
const MAX_LIFETIME: u64 = 8 * 24 * 3600;
/// How far ahead of the relay's clock a token's `iat` may be.
const SKEW: u64 = 60;
/// How often the key and denylist files are looked at for a change.
const WATCH_EVERY: Duration = Duration::from_secs(5);

/// The `[claktory]` table of claktory-relay.toml.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub mode: Mode,
    /// The public keys that sign relay tokens: {"seq", "keys": [64 hex]}.
    pub keys: PathBuf,
    /// The accounts refused: {"seq", "a": [32 hex]}.
    pub denylist: PathBuf,
}

/// Whether a packet no signed-in Claktory sends or receives is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Forward it, and count it.
    Observe,
    /// Drop it, and count it.
    Enforce,
}

/// An account's opaque id in tokens (`a`): HMAC of the account id.
pub type AccountTag = [u8; 16];

/// Why a token gave no authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejected {
    /// Malformed, too long, or not signed by a key in the key file.
    Bad,
    /// Past its `exp`.
    Expired,
    /// Bound to another endpoint.
    Endpoint,
    /// Its account is on the denylist.
    Denied,
}

/// What claktory-relay adds to the relay's metrics.
#[derive(Debug, Default, MetricsGroup)]
#[metrics(name = "claktory_relay")]
#[non_exhaustive]
pub struct Metrics {
    #[metrics(help = "Endpoints connected with a valid relay token.")]
    pub clients_authorized: Gauge,
    #[metrics(help = "Endpoints connected without a valid relay token.")]
    pub clients_unauthorized: Gauge,
    #[metrics(help = "Packets between two endpoints without a token: dropped in enforce, forwarded in observe.")]
    pub forward_denied_packets: Counter,
    #[metrics(help = "Bytes of forward_denied_packets.")]
    pub forward_denied_bytes: Counter,
    #[metrics(help = "Relay tokens accepted.")]
    pub tokens_accepted: Counter,
    #[metrics(help = "Relay tokens refused as malformed or wrongly signed.")]
    pub tokens_rejected_bad: Counter,
    #[metrics(help = "Relay tokens refused as expired.")]
    pub tokens_rejected_expired: Counter,
    #[metrics(help = "Relay tokens refused as bound to another endpoint.")]
    pub tokens_rejected_endpoint: Counter,
    #[metrics(help = "Relay tokens refused for a denied account.")]
    pub tokens_rejected_denied: Counter,
    #[metrics(help = "1 while enforcing, 0 while observing.")]
    pub enforcing: Gauge,
    #[metrics(help = "Token-signing keys loaded.")]
    pub keys_loaded: Gauge,
    #[metrics(help = "Accounts on the denylist.")]
    pub denylist_entries: Gauge,
    #[metrics(help = "Key file, denylist or mode reloads refused.")]
    pub reload_failures: Counter,
}

/// The key file, parsed.
#[derive(Debug, Clone)]
pub struct Keys {
    seq: u64,
    keys: Vec<PublicKey>,
}

/// The denylist, parsed.
#[derive(Debug, Clone, Default)]
pub struct Denylist {
    seq: u64,
    accounts: HashSet<AccountTag>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KeysFile {
    seq: u64,
    keys: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DenylistFile {
    seq: u64,
    a: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    v: u8,
    e: String,
    a: String,
    iat: u64,
    exp: u64,
}

/// `N` bytes written as exactly 2N lowercase hex digits.
fn hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let digits = text.as_bytes();
    if digits.len() != 2 * N {
        return None;
    }
    let value = |digit: u8| match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; N];
    for (byte, pair) in out.iter_mut().zip(digits.chunks_exact(2)) {
        *byte = value(pair[0])? << 4 | value(pair[1])?;
    }
    Some(out)
}

/// A key file's contents: 1 or 2 Ed25519 public keys.
pub fn parse_keys(bytes: &[u8]) -> Result<Keys, String> {
    if bytes.len() as u64 > MAX_KEYS_FILE {
        return Err(format!("over {MAX_KEYS_FILE} bytes"));
    }
    let file: KeysFile = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if !(1..=2).contains(&file.keys.len()) {
        return Err(format!("{} keys, not 1 or 2", file.keys.len()));
    }
    let keys = file
        .keys
        .iter()
        .map(|key| hex::<32>(key).and_then(|bytes| PublicKey::from_bytes(&bytes).ok()).ok_or_else(|| format!("not a public key: {key:?}")))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Keys { seq: file.seq, keys })
}

/// A denylist's contents: at most `MAX_DENIED` accounts, none at all too.
pub fn parse_denylist(bytes: &[u8]) -> Result<Denylist, String> {
    if bytes.len() as u64 > MAX_DENYLIST_FILE {
        return Err(format!("over {MAX_DENYLIST_FILE} bytes"));
    }
    let file: DenylistFile = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if file.a.len() > MAX_DENIED {
        return Err(format!("{} accounts, more than {MAX_DENIED}", file.a.len()));
    }
    let accounts = file.a.iter().map(|a| hex::<16>(a).ok_or_else(|| format!("not an account: {a:?}"))).collect::<Result<_, _>>()?;
    Ok(Denylist { seq: file.seq, accounts })
}

/// The account a relay token names, if it's good for `endpoint` at `now`
/// (unix seconds).
pub fn verify(
    token: &str,
    endpoint: EndpointId,
    keys: &[PublicKey],
    denied: &HashSet<AccountTag>,
    now: u64,
) -> Result<AccountTag, Rejected> {
    if token.len() > MAX_TOKEN {
        return Err(Rejected::Bad);
    }
    let (payload, signature) = token.split_once('.').ok_or(Rejected::Bad)?;
    let payload = URL_SAFE_NO_PAD.decode(payload).map_err(|_| Rejected::Bad)?;
    let signature: [u8; Signature::LENGTH] =
        URL_SAFE_NO_PAD.decode(signature).map_err(|_| Rejected::Bad)?.try_into().map_err(|_| Rejected::Bad)?;
    let signature = Signature::from_bytes(&signature);
    let signed = [DOMAIN, &payload].concat();
    if !keys.iter().any(|key| key.verify(&signed, &signature).is_ok()) {
        return Err(Rejected::Bad);
    }
    let payload: Payload = serde_json::from_slice(&payload).map_err(|_| Rejected::Bad)?;
    let account = hex::<16>(&payload.a).ok_or(Rejected::Bad)?;
    let bound = hex::<32>(&payload.e).ok_or(Rejected::Bad)?;
    if payload.v != 1 || payload.exp <= payload.iat || payload.exp - payload.iat > MAX_LIFETIME || payload.iat > now + SKEW {
        return Err(Rejected::Bad);
    }
    if now >= payload.exp {
        return Err(Rejected::Expired);
    }
    if &bound != endpoint.as_bytes() {
        return Err(Rejected::Endpoint);
    }
    if denied.contains(&account) {
        return Err(Rejected::Denied);
    }
    Ok(account)
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|since| since.as_secs()).unwrap_or(0)
}

/// Reads at most `limit` bytes of `path`, and fails past it.
fn read_capped(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{}: over {limit} bytes", path.display()));
    }
    Ok(bytes)
}

/// claktory-relay's access control and forwarding rule.
#[derive(Debug)]
pub struct ClaktoryAccess {
    enforcing: AtomicBool,
    keys: RwLock<Option<Keys>>,
    denylist: RwLock<Denylist>,
    /// Every connection, by endpoint, with the account of its good token.
    connections: Mutex<HashMap<EndpointId, HashMap<ConnectionId, Option<AccountTag>>>>,
    /// The endpoints with at least one connection that showed a good token:
    /// what forwarding reads, for every packet.
    authorized: RwLock<HashSet<EndpointId>>,
    metrics: Arc<Metrics>,
    clock: fn() -> u64,
}

impl ClaktoryAccess {
    /// Observing, with no keys and nobody denied.
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self::with_clock(metrics, unix_now)
    }

    fn with_clock(metrics: Arc<Metrics>, clock: fn() -> u64) -> Self {
        Self {
            enforcing: AtomicBool::new(false),
            keys: RwLock::default(),
            denylist: RwLock::default(),
            connections: Mutex::default(),
            authorized: RwLock::default(),
            metrics,
            clock,
        }
    }

    /// As `config` says, its files read. Enforcing needs both files good;
    /// observing starts without them, saying so.
    pub fn start(config: &Config, metrics: Arc<Metrics>) -> Result<Arc<Self>, String> {
        let access = Arc::new(Self::new(metrics));
        let keys = access.load_keys(&config.keys);
        let denylist = access.load_denylist(&config.denylist);
        if config.mode == Mode::Enforce {
            keys?;
            denylist?;
        } else {
            for error in [keys, denylist].into_iter().filter_map(Result::err) {
                warn!("observing without it: {error}");
            }
        }
        access.set_mode(config.mode)?;
        Ok(access)
    }

    pub fn mode(&self) -> Mode {
        if self.enforcing.load(Ordering::Relaxed) { Mode::Enforce } else { Mode::Observe }
    }

    /// Switches mode; never to enforce without a key, which would cut every
    /// Claktory off.
    pub fn set_mode(&self, mode: Mode) -> Result<(), String> {
        if mode == Mode::Enforce && self.keys.read().unwrap().is_none() {
            self.metrics.reload_failures.inc();
            return Err("won't enforce without a valid key file".to_owned());
        }
        if mode != self.mode() {
            info!(?mode, "relay access mode");
        }
        self.enforcing.store(mode == Mode::Enforce, Ordering::Relaxed);
        self.metrics.enforcing.set(i64::from(mode == Mode::Enforce));
        Ok(())
    }

    /// Reads the key file. A bad one, or one whose seq is lower than the
    /// loaded one's, leaves the loaded keys in place.
    pub fn load_keys(&self, path: &Path) -> Result<(), String> {
        let result = read_capped(path, MAX_KEYS_FILE).and_then(|bytes| parse_keys(&bytes)).and_then(|keys| {
            let mut loaded = self.keys.write().unwrap();
            if let Some(old) = loaded.as_ref()
                && keys.seq < old.seq
            {
                return Err(format!("seq {} is older than the loaded {}", keys.seq, old.seq));
            }
            info!(seq = keys.seq, keys = keys.keys.len(), "relay token keys loaded");
            self.metrics.keys_loaded.set(keys.keys.len() as i64);
            *loaded = Some(keys);
            Ok(())
        });
        result.map_err(|error| {
            self.metrics.reload_failures.inc();
            format!("key file {}: {error}", path.display())
        })
    }

    /// Reads the denylist, and takes authorization away at once from every
    /// connection whose account it names. A bad one, or one whose seq is
    /// lower than the loaded one's, leaves the loaded one in place.
    pub fn load_denylist(&self, path: &Path) -> Result<(), String> {
        let result = read_capped(path, MAX_DENYLIST_FILE).and_then(|bytes| parse_denylist(&bytes)).and_then(|denylist| {
            let mut loaded = self.denylist.write().unwrap();
            if denylist.seq < loaded.seq {
                return Err(format!("seq {} is older than the loaded {}", denylist.seq, loaded.seq));
            }
            info!(seq = denylist.seq, accounts = denylist.accounts.len(), "relay denylist loaded");
            self.metrics.denylist_entries.set(denylist.accounts.len() as i64);
            *loaded = denylist;
            Ok(())
        });
        match result {
            Ok(()) => {
                self.apply_denylist();
                Ok(())
            }
            Err(error) => {
                self.metrics.reload_failures.inc();
                Err(format!("denylist {}: {error}", path.display()))
            }
        }
    }

    fn apply_denylist(&self) {
        let denied = self.denylist.read().unwrap().accounts.clone();
        let mut connections = self.connections.lock().unwrap();
        let endpoints: Vec<EndpointId> = connections.keys().copied().collect();
        for endpoint in endpoints {
            self.change(&mut connections, endpoint, |held| {
                for account in held.values_mut() {
                    if account.is_some_and(|account| denied.contains(&account)) {
                        info!(endpoint = %endpoint.fmt_short(), "authorization withdrawn: its account is denied");
                        *account = None;
                    }
                }
            });
        }
    }

    /// Changes `endpoint`'s connections with `edit`, keeping the authorized
    /// set and the client gauges in step.
    fn change(
        &self,
        connections: &mut HashMap<EndpointId, HashMap<ConnectionId, Option<AccountTag>>>,
        endpoint: EndpointId,
        edit: impl FnOnce(&mut HashMap<ConnectionId, Option<AccountTag>>),
    ) {
        let status = |held: Option<&HashMap<ConnectionId, Option<AccountTag>>>| {
            held.filter(|held| !held.is_empty()).map(|held| held.values().any(Option::is_some))
        };
        let before = status(connections.get(&endpoint));
        let held = connections.entry(endpoint).or_default();
        edit(held);
        if held.is_empty() {
            connections.remove(&endpoint);
        }
        let after = status(connections.get(&endpoint));
        if before == after {
            return;
        }
        let gauge = |authorized: bool| if authorized { &self.metrics.clients_authorized } else { &self.metrics.clients_unauthorized };
        if let Some(authorized) = before {
            gauge(authorized).dec();
        }
        if let Some(authorized) = after {
            gauge(authorized).inc();
        }
        let mut set = self.authorized.write().unwrap();
        if after == Some(true) {
            set.insert(endpoint);
        } else {
            set.remove(&endpoint);
        }
    }

    /// Whether `endpoint` holds a connection that showed a good token.
    #[cfg(test)]
    pub fn is_authorized(&self, endpoint: EndpointId) -> bool {
        self.authorized.read().unwrap().contains(&endpoint)
    }

    /// The account `token` names if it's good for `endpoint` now, counted.
    fn check(&self, token: &str, endpoint: EndpointId) -> Option<AccountTag> {
        let keys = self.keys.read().unwrap();
        let keys = keys.as_ref().map(|keys| keys.keys.as_slice()).unwrap_or_default();
        let verdict = verify(token, endpoint, keys, &self.denylist.read().unwrap().accounts, (self.clock)());
        match verdict {
            Ok(account) => {
                self.metrics.tokens_accepted.inc();
                debug!(endpoint = %endpoint.fmt_short(), account = %HexTag(&account), "relay token accepted");
                Some(account)
            }
            Err(why) => {
                let counter = match why {
                    Rejected::Bad => &self.metrics.tokens_rejected_bad,
                    Rejected::Expired => &self.metrics.tokens_rejected_expired,
                    Rejected::Endpoint => &self.metrics.tokens_rejected_endpoint,
                    Rejected::Denied => &self.metrics.tokens_rejected_denied,
                };
                counter.inc();
                // Never the token itself.
                debug!(endpoint = %endpoint.fmt_short(), ?why, "relay token refused");
                None
            }
        }
    }

    fn connected(&self, endpoint: EndpointId, connection: ConnectionId, token: Option<&str>) {
        let account = token.and_then(|token| self.check(token, endpoint));
        let mut connections = self.connections.lock().unwrap();
        self.change(&mut connections, endpoint, |held| {
            held.insert(connection, account);
        });
    }

    fn disconnected(&self, endpoint: EndpointId, connection: ConnectionId) {
        let mut connections = self.connections.lock().unwrap();
        self.change(&mut connections, endpoint, |held| {
            held.remove(&connection);
        });
    }

    /// Looks at the key file and the denylist every few seconds and reads
    /// one again when it changed (the deploy replaces it with a rename, so
    /// it's its path that's watched, never an open file).
    pub async fn watch(self: Arc<Self>, config: Config) {
        self.watch_every(config, WATCH_EVERY).await
    }

    async fn watch_every(self: Arc<Self>, config: Config, period: Duration) {
        let stamp = |path: &Path| {
            std::fs::metadata(path).ok().map(|meta| {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino(), meta.len(), meta.mtime(), meta.mtime_nsec())
            })
        };
        let mut seen = (stamp(&config.keys), stamp(&config.denylist));
        let mut every = tokio::time::interval(period);
        every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            every.tick().await;
            let now = (stamp(&config.keys), stamp(&config.denylist));
            if now.0 != seen.0
                && now.0.is_some()
                && let Err(error) = self.load_keys(&config.keys)
            {
                warn!("{error}");
            }
            if now.1 != seen.1
                && now.1.is_some()
                && let Err(error) = self.load_denylist(&config.denylist)
            {
                warn!("{error}");
            }
            seen = now;
        }
    }

    /// Reads both files again, and the mode (SIGHUP).
    pub fn reload(&self, config: &Config) {
        for result in [self.load_keys(&config.keys), self.load_denylist(&config.denylist), self.set_mode(config.mode)] {
            if let Err(error) = result {
                warn!("{error}");
            }
        }
    }
}

/// An account tag in logs: hex.
struct HexTag<'a>(&'a AccountTag);

impl std::fmt::Display for HexTag<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl AccessControl for ClaktoryAccess {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        self.connected(request.endpoint_id(), request.connection_id(), request.auth_token().as_deref());
        Access::Allow
    }

    fn on_disconnect(&self, endpoint_id: EndpointId, connection_id: ConnectionId) {
        self.disconnected(endpoint_id, connection_id);
    }

    fn allow_forward(&self, src: EndpointId, dst: EndpointId, bytes: usize) -> bool {
        {
            let authorized = self.authorized.read().unwrap();
            if authorized.contains(&src) || authorized.contains(&dst) {
                return true;
            }
        }
        self.metrics.forward_denied_packets.inc();
        self.metrics.forward_denied_bytes.inc_by(bytes as u64);
        !self.enforcing.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests;
