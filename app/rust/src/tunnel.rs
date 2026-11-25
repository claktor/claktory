//! The private tunnel's app side (docs: private-tunnel.md): the app reaches
//! its backend only through iroh. A [`Tunnel`] listens on a random loopback
//! port; each TCP connection the app opens there (one WebSocket) becomes a
//! new bidirectional stream on one iroh connection to the backend, carrying
//! exactly the socket's bytes both ways. The protocol code above it is the
//! same as over a plain socket. A backend that has `claktory/2` gets them
//! compressed (zstd); an older one, as they are (`claktory/1`).
//!
//! The port is this device's, not this app's: any program on it can
//! connect there (another app on an iPhone, any process or user on a Mac),
//! and whatever passes would reach the backend as this device. So each
//! tunnel has a secret, and only a WebSocket asking for its path
//! (`ws://127.0.0.1:<port>/<secret>`) is forwarded; anything else is
//! closed before it reaches iroh.

use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use async_compression::tokio::{bufread::ZstdDecoder, write::ZstdEncoder};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayUrl, SecretKey, Watcher,
    endpoint::{ConnectError, ConnectOptions, Connection, ConnectionError, presets},
};
use tokio::{
    io::{AsyncWriteExt, BufReader, copy},
    net::{TcpListener, TcpStream},
    sync::Mutex as AsyncMutex,
    task::JoinHandle,
};

/// The backend's protocol on iroh, both sides (private-tunnel.md).
pub const ALPN: &[u8] = b"claktory/1";
/// The same bytes, each way one zstd stream flushed whenever what's written
/// is (the WebSocket flushes after every message). Offered first; backends
/// before it answer with [ALPN].
pub const ALPN_COMPRESSED: &[u8] = b"claktory/2";

/// How long a dial may take before the app says why it can't connect,
/// rather than waiting on, the first time: under the app's own connect
/// window, so a slow dial fails here first and the next try isn't queued
/// behind it.
pub const DIAL_TIMEOUT: Duration = DIAL_TIMEOUTS[0];

/// How long each dial has, in turn, after the ones before it timed out:
/// on a weak link a dial that's getting there is let finish rather than
/// cut at the same point every time (#413: Matt's phone, on one or two
/// bars, gave up 27 dials a day at exactly 8 s, and every cut starts the
/// handshake from zero). Back to the first once one is made. The app's
/// connect window follows ([Status::dial_timeout_ms]).
#[cfg(not(test))]
pub const DIAL_TIMEOUTS: [Duration; 3] = [Duration::from_secs(8), Duration::from_secs(16), Duration::from_secs(30)];
#[cfg(test)]
pub const DIAL_TIMEOUTS: [Duration; 3] = [Duration::from_millis(400), Duration::from_millis(800), Duration::from_millis(1500)];

/// How many dials to each backend (by its endpoint id) have timed out in a
/// row: kept across tunnels, since the app opens a new one when it starts
/// over from scratch, and patience is about the link, not the tunnel.
static DIALS_TIMED_OUT: Mutex<Vec<(EndpointId, usize)>> = Mutex::new(Vec::new());

/// How long an endpoint may go without a home relay chosen before that
/// counts as trouble ([RelayTrouble::Unchosen]): its first measurement of
/// the network takes a second or two, and it chooses none until then.
pub const UNCHOSEN_AFTER: Duration = Duration::from_secs(5);

/// How long a connection made on a direct path has to be confirmed by the
/// backend (QUIC's HANDSHAKE_DONE) before the device takes it for a stall
/// and dials again through the relay (#392). A path can answer one way only:
/// a Mac backend reached at an address that isn't its route back to the
/// device answers from that route's address (noq-udp asks macOS for the
/// source address in a way it ignores). iroh lets the device follow it
/// there mid-handshake, but the backend's half of the handshake stays
/// pinned to the address the device's first packet reached, so it discards
/// all the device sends next: the device thinks it's connected, the
/// backend never does, and the relay's copy of the first packet, arriving
/// later, is discarded as a duplicate. A working direct path confirms
/// within one round trip, a few to a few hundred milliseconds.
pub const CONFIRM_WITHIN: Duration = Duration::from_millis(1500);

/// How long a socket has to say what it asks for, and how much it may say
/// before its request line ends.
#[cfg(not(test))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
const REQUEST_LINE_MAX: usize = 1024;

/// How long the old endpoint has to close before a new one is made in its
/// place, or [Tunnel::close] returns: its sockets may be dead (iOS reclaims
/// a suspended app's), so it's not waited on longer. One that hasn't
/// closed by then is said so in the log and finishes closing in the
/// background ([close_endpoint]); the new one may meet it at the relay for
/// that moment, where the newer wins.
#[cfg(not(test))]
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);
/// Under iroh's own drain (about 3 s, its probe timeout), so the tests see
/// the slow path every time rather than half the time.
#[cfg(test)]
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// An endpoint younger than this isn't made afresh by [Tunnel::reconnect]:
/// it was just made, and timeouts that call reconnect in a row mustn't make
/// one every few seconds.
pub const REBUILD_AFTER: Duration = Duration::from_secs(5);

/// The longest [REBUILD_AFTER] grows to while rebuilds reach nothing: an
/// app retrying in the background (Doze cuts its network) calls reconnect
/// every few seconds, and a new endpoint each time would only cost battery.
const REBUILD_AFTER_MAX: Duration = Duration::from_secs(300);

/// How old the endpoint must be before reconnect makes it afresh, after
/// [unanswered] rebuilds in a row that no connection followed: [base],
/// doubled for each, up to [REBUILD_AFTER_MAX].
fn rebuild_age(base: Duration, unanswered: u32) -> Duration {
    base.saturating_mul(1 << unanswered.min(8)).min(REBUILD_AFTER_MAX.max(base))
}

/// How long making the device's endpoint may take (#214): binding its
/// sockets and starting its tasks is quick, so one that takes longer is
/// stuck (on a phone back from suspension, say) and fails, for the app to
/// try again, rather than holding the endpoint's slot, and every dial and
/// close waiting on it, for ever.
#[cfg(not(test))]
const MAKE_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const MAKE_TIMEOUT: Duration = Duration::from_secs(1);

/// How long telling the endpoint the network changed may take (#214).
const NETWORK_CHANGE_TIMEOUT: Duration = Duration::from_secs(5);

/// [make]'s endpoint, or a failure once it has taken [MAKE_TIMEOUT].
async fn make_within(make: &MakeEndpoint) -> anyhow::Result<Endpoint> {
    match tokio::time::timeout(MAKE_TIMEOUT, make()).await {
        Ok(made) => made,
        Err(_) => anyhow::bail!("this device's side of the tunnel took over {} s to start", MAKE_TIMEOUT.as_secs()),
    }
}

/// Makes the device's endpoint, the first time and after [Tunnel::reconnect].
pub type MakeEndpoint = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = anyhow::Result<Endpoint>> + Send>> + Send + Sync>;

/// A new device key: 32 random bytes as lowercase hex, made on the device.
pub fn generate_key() -> String {
    hex::encode(SecretKey::generate().to_bytes())
}

/// The secret key [hex] stands for.
pub fn parse_key(hex: &str) -> anyhow::Result<SecretKey> {
    let bytes: [u8; 32] = hex::decode(hex.trim())?.try_into().map_err(|_| anyhow::anyhow!("a device key is 32 bytes"))?;
    Ok(SecretKey::from_bytes(&bytes))
}

/// The endpoint ids a pairing number is worked out from, as lowercase hex:
/// this device's, from its [device_key], and the backend's [endpoint_id]
/// (hex or base32, as in a pairing code's `e=`).
pub fn pairing_ids(device_key: &str, endpoint_id: &str) -> anyhow::Result<(String, String)> {
    let device = parse_key(device_key)?.public();
    let backend: EndpointId = endpoint_id.trim().to_lowercase().parse().map_err(|error| anyhow::anyhow!("not an endpoint id: {error}"))?;
    Ok((hex::encode(device.as_bytes()), hex::encode(backend.as_bytes())))
}

/// Where the backend is: its endpoint id, the relays its pairing code
/// names (none: iroh finds it through its address lookup and n0's relays),
/// and the direct UDP addresses it names (`a=`: this Mac's own code has
/// only 127.0.0.1:<port>, and no relay).
pub fn backend_addr(endpoint_id: &str, relays: &[String], addresses: &[String]) -> anyhow::Result<EndpointAddr> {
    let id: EndpointId = endpoint_id.trim().parse().map_err(|error| anyhow::anyhow!("not an endpoint id: {error}"))?;
    let mut addr = EndpointAddr::new(id);
    for relay in relays {
        let url: RelayUrl = relay.parse().map_err(|error| anyhow::anyhow!("not a relay URL ({relay}): {error}"))?;
        addr = addr.with_relay_url(url);
    }
    for address in addresses {
        let socket: SocketAddr = address.trim().parse().map_err(|error| anyhow::anyhow!("not an address ({address}): {error}"))?;
        addr = addr.with_ip_addr(socket);
    }
    Ok(addr)
}

/// Claktory's own relay (relay.claktory.com, Matt's choice of 21 Nov): the
/// device's home relay. Written exactly as factoryd's default writes it
/// (no dot after the host): iroh tells relays apart by the URL as written,
/// so `relay.claktory.com./` would be another relay to it, and the device
/// would hold two connections to the one server, each displacing the other.
pub const CLAKTORY_RELAY: &str = "https://relay.claktory.com/";

/// A device endpoint with [key], homed on Claktory's relay, with iroh's
/// address lookup, so a backend whose home relay changed is still found.
///
/// Homed there rather than on n0's (#388): n0's map makes the device probe
/// every region before it picks one (1 to 1.4 s on a good network against
/// 0.35 s, app/rust/examples/relay_timing.rs, and far longer on a slow
/// one). The device's home relay isn't what it dials through: a dial goes
/// at once through the relay the backend's pairing code names (or the one
/// the lookup finds), n0's or ours, whatever this map holds.
///
/// It looks backends up as n0's preset does (pkarr over HTTPS, and DNS),
/// but never publishes its own address: nothing dials a device, and the
/// backend reaches it on the connection it made (#413: ~5 KB for every
/// endpoint made afresh, and again every 5 minutes, on a phone's data).
pub async fn device_endpoint(key: SecretKey) -> anyhow::Result<Endpoint> {
    use iroh::address_lookup::{DnsAddressLookup, PkarrResolver};
    Ok(Endpoint::builder(presets::Minimal)
        .address_lookup(PkarrResolver::n0_dns())
        .address_lookup(DnsAddressLookup::n0_dns())
        .relay_mode(device_relays())
        .secret_key(key)
        .transport_config(device_transport())
        .bind()
        .await?)
}

/// The device's relays: Claktory's only.
fn device_relays() -> iroh::RelayMode {
    let ours: RelayUrl = CLAKTORY_RELAY.parse().expect("Claktory's relay URL");
    iroh::RelayMode::Custom(iroh::RelayMap::from(ours))
}

/// iroh's QUIC settings, but on Android without segmentation offload
/// (GSO: a burst of packets handed to the kernel in one send). Android
/// is the only device that has it, and on the Android emulator such a
/// burst vanished without an error, so every upload of more than a few KB
/// on the wire stalled until the connection was lost, while small
/// messages and everything downloaded got through; the same build with
/// GSO off sent them all (Doppio's matched A/B on 53638636: 5,698 B and
/// 400 KiB random files lost, then delivered byte for byte;
/// app/android/DESIGN.md §3). Nothing tells us every phone's network
/// driver handles it either, and a phone's uploads are small, so sending
/// packet by packet costs nothing that matters. The Mac and the iPhone
/// keep iroh's defaults (they never use GSO anyway).
fn device_transport() -> iroh::endpoint::QuicTransportConfig {
    transport_for(cfg!(target_os = "android"))
}

fn transport_for(android: bool) -> iroh::endpoint::QuicTransportConfig {
    iroh::endpoint::QuicTransportConfig::builder().enable_segmentation_offload(!android).build()
}

/// What the tunnel is doing, for the app to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub phase: Phase,
    /// How the open connection travels; [Path::None] while there's none.
    pub path: Path,
    /// Why the last dial failed, or the connection closed: a sentence for
    /// the person. Empty while all's well.
    pub error: String,
    /// What kind of failure [error] is, so the app can say what to do.
    pub failure: Failure,
    /// Whether this device reaches its home relay now.
    pub relay_connected: bool,
    /// How long its home relay took to come up, in ms, from when the
    /// endpoint was made or the relay was last lost; 0 while it's down
    /// (#388: diagnostics, to see where a slow connect goes).
    pub relay_up_ms: u32,
    /// Why its home relay can't be reached, roughly, while it can't.
    pub relay_trouble: RelayTrouble,
    /// Bytes received from the backend so far, on every stream, as they
    /// arrive (before they're decompressed), wrapping: while it moves, the
    /// link is delivering, even if no whole message has arrived yet (#413:
    /// the app's ping, its pong queued behind a slow Workspace, closed the
    /// connection that was bringing it, every 12 s, for ever).
    pub received: u32,
    /// Bytes in the UDP datagrams the open connection has received, QUIC's
    /// own included (acknowledgements, heartbeats, retransmissions),
    /// wrapping; it jumps when a new connection is made. On a weak link
    /// whose queue makes QUIC back off, stream data can pause for longer
    /// than the app's ping allows while the connection is plainly alive
    /// (Sale's 63 kbps runs, #413): this still moves then, and stops only
    /// when nothing arrives at all.
    pub link_received: u32,
    /// How long the next dial has ([DIAL_TIMEOUTS]), in ms: the app waits
    /// at least that long for its connection through the tunnel.
    pub dial_timeout_ms: u32,
    /// What kind of network the direct path crosses, from its address
    /// (never the address itself); [Reach::None] while the path isn't
    /// direct.
    pub reach: Reach,
}

/// What a direct path crosses, from the other side's address (Polpo's
/// efficiency study §4: per route, LAN or tailnet or the internet).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Reach {
    /// Not direct, or not connected.
    #[default]
    None,
    /// This machine itself (loopback).
    Local,
    /// A private or link-local address: the same network.
    Lan,
    /// Tailscale's CGNAT range (100.64.0.0/10) or its IPv6 prefix.
    Tailnet,
    /// Any other address: across the internet, a hole punched.
    Public,
}

impl Reach {
    pub fn of(addr: &std::net::SocketAddr) -> Self {
        use std::net::IpAddr;
        match addr.ip().to_canonical() {
            IpAddr::V4(ip) => {
                let [a, b, ..] = ip.octets();
                if ip.is_loopback() {
                    Reach::Local
                } else if a == 100 && (64..128).contains(&b) {
                    Reach::Tailnet
                } else if ip.is_private() || ip.is_link_local() {
                    Reach::Lan
                } else {
                    Reach::Public
                }
            }
            IpAddr::V6(ip) => {
                let s = ip.segments();
                if ip.is_loopback() {
                    Reach::Local
                } else if s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0 {
                    Reach::Tailnet
                } else if (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80 {
                    Reach::Lan
                } else {
                    Reach::Public
                }
            }
        }
    }
}

/// Why a home relay can't be reached, in a word (#388): from iroh's last
/// error for it, which says more but may name hosts, so only its class is
/// kept.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RelayTrouble {
    /// None known: connected, or no try has failed yet.
    #[default]
    None,
    /// No home relay chosen for [UNCHOSEN_AFTER]: iroh picks one from its
    /// measurement of the network (its net report), and the last found
    /// none it could reach. It measures again 20 to 26 s later, or when
    /// the network changes. Before that, while the first measurement runs,
    /// there's no trouble to tell (Zucchero: every launch said so).
    Unchosen,
    /// Its name couldn't be looked up.
    Dns,
    /// Nothing answered in time.
    Timeout,
    /// The secure connection to it failed.
    Tls,
    /// The network turned it away: refused, reset or unreachable.
    Network,
    Other,
}

impl RelayTrouble {
    /// The class of [said], an iroh relay error with its causes.
    fn of(said: &str) -> Self {
        let said = said.to_lowercase();
        let any = |signs: &[&str]| signs.iter().any(|sign| said.contains(sign));
        if any(&["dns", "resolve", "lookup", "no addresses", "nodename", "name or service"]) {
            RelayTrouble::Dns
        } else if any(&["timeout", "timed out", "elapsed", "deadline"]) {
            RelayTrouble::Timeout
        } else if any(&["tls", "certificate", "handshake"]) {
            RelayTrouble::Tls
        } else if any(&["refused", "reset", "unreachable", "no route", "network is down", "not connected"]) {
            RelayTrouble::Network
        } else {
            RelayTrouble::Other
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    /// No connection yet: the first socket dials.
    #[default]
    Idle,
    Dialing,
    Connected,
    /// The last dial failed, or the connection closed; the next socket
    /// dials again.
    Failed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Path {
    #[default]
    None,
    /// Straight between the two machines (same machine, LAN, or a hole
    /// punched through the NATs).
    Direct,
    /// Through a relay, end-to-end encrypted.
    Relayed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Failure {
    #[default]
    None,
    /// Nothing answered: the backend's machine is off, asleep or offline,
    /// or remote access is off there.
    Unreachable,
    /// The backend answered and turned this device away.
    Refused,
    /// This device has no network, or can't reach any relay.
    Offline,
    /// Anything else, said as it is.
    Other,
}

/// The tunnel: a loopback port the app connects to, forwarding to the
/// backend over iroh.
pub struct Tunnel {
    port: u16,
    /// What the app asks for to be forwarded: `/<secret>`, 32 random bytes
    /// in hex, new for every tunnel.
    path: String,
    shared: Arc<Shared>,
    accept: JoinHandle<()>,
}

struct Shared {
    /// The device's endpoint, made afresh by [Tunnel::reconnect]; None
    /// while that's under way, or when making it failed (the next dial
    /// tries again). Held while it's made, so a dial waits for it.
    endpoint: AsyncMutex<Slot>,
    /// Makes it; None when it can only be told the network changed.
    make: Option<MakeEndpoint>,
    /// How old an endpoint must be before reconnect makes a new one.
    rebuild_after: Duration,
    /// Rebuilds since the last connection made, to back off ([rebuild_age]).
    unanswered: AtomicU32,
    /// Held by the [Tunnel::reconnect] under way: another one joins it.
    reconnecting: AsyncMutex<()>,
    /// When a connection was last made: the endpoint works, whatever its
    /// age, and isn't made afresh for [rebuild_age] after.
    connected_at: Mutex<Option<Instant>>,
    /// Follows the current endpoint's home relay.
    relay_watch: Mutex<Option<JoinHandle<()>>>,
    backend: EndpointAddr,
    /// Set once a direct path stalled ([CONFIRM_WITHIN]): dials go through
    /// the backend's relay only, and iroh upgrades the connection to a
    /// direct path it has checked both ways. Cleared when such a dial
    /// fails, so a network where only the code's addresses work (no
    /// internet, a LAN or a VPN) still gets them on the next.
    relay_first: AtomicBool,
    /// The connection streams open on; dialled by the first socket that
    /// finds none open.
    connection: AsyncMutex<Option<Connection>>,
    status: Mutex<Status>,
    /// The connection localhost forwards travel on (`crate::forward`),
    /// dialled by the first one.
    forwards: AsyncMutex<Option<Connection>>,
    /// Tells a dial in flight to give up: the network changed under it
    /// (`reconnect`), so it may be stuck on a dead path.
    cancel_dials: tokio::sync::Notify,
    /// [Status::received].
    received: AtomicU32,
    /// [Status::link_received]: the open connection's, as [follow] last
    /// read it.
    link_received: AtomicU32,
}

/// Closes [endpoint], waiting [CLOSE_TIMEOUT] at most. One that takes
/// longer (its connections draining on a network that's gone) finishes
/// closing in the background: dropped half-closed, iroh would never stop
/// its tasks or its relay connection (its abort does nothing once a close
/// has begun). Until it has, the relay keeps the newer endpoint with this
/// key as the one it delivers to.
async fn close_endpoint(endpoint: Endpoint) {
    let begun = Instant::now();
    let mut closing = tokio::spawn(async move { endpoint.close().await });
    if tokio::time::timeout(CLOSE_TIMEOUT, &mut closing).await.is_err() {
        // Logcat's only (android.rs): nothing reads it elsewhere. Both
        // lines, so how long two endpoints with this key lived can be told.
        #[cfg(any(target_os = "android", test))]
        tracing::warn!("The old endpoint didn't close within {} s; it finishes closing in the background.", CLOSE_TIMEOUT.as_secs());
        tokio::spawn(async move {
            let _ = closing.await;
            #[cfg(any(target_os = "android", test))]
            tracing::warn!("The old endpoint finished closing in the background, {:.1} s after it began.", begun.elapsed().as_secs_f64());
            #[cfg(not(any(target_os = "android", test)))]
            let _ = begun;
        });
    }
}

/// The current endpoint and when it was made.
struct Slot {
    endpoint: Option<Endpoint>,
    made: Instant,
    /// Set by [Tunnel::close]: nothing is made any more.
    closed: bool,
}

impl Tunnel {
    /// Listens on 127.0.0.1 at a random port and forwards to [backend]
    /// through [endpoint]. Nothing is dialled until the app connects.
    /// [reconnect](Self::reconnect) can only tell this endpoint the network
    /// changed; [open_with](Self::open_with) can make it afresh.
    pub async fn open(endpoint: Endpoint, backend: EndpointAddr) -> anyhow::Result<Self> {
        Self::start(endpoint, None, REBUILD_AFTER, backend).await
    }

    /// As [open](Self::open), with an endpoint [make] makes, now and again
    /// whenever [reconnect](Self::reconnect) finds it older than
    /// [rebuild_after].
    pub async fn open_with(make: MakeEndpoint, rebuild_after: Duration, backend: EndpointAddr) -> anyhow::Result<Self> {
        let endpoint = make_within(&make).await?;
        Self::start(endpoint, Some(make), rebuild_after, backend).await
    }

    async fn start(endpoint: Endpoint, make: Option<MakeEndpoint>, rebuild_after: Duration, backend: EndpointAddr) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
        let port = listener.local_addr()?.port();
        // A key's 32 bytes are as random as any: the system's CSPRNG.
        let path: Arc<str> = format!("/{}", hex::encode(SecretKey::generate().to_bytes())).into();
        let shared = Arc::new(Shared {
            endpoint: AsyncMutex::new(Slot { endpoint: Some(endpoint.clone()), made: Instant::now(), closed: false }),
            make,
            rebuild_after,
            unanswered: AtomicU32::new(0),
            reconnecting: AsyncMutex::new(()),
            connected_at: Mutex::new(None),
            relay_watch: Mutex::new(None),
            backend,
            relay_first: AtomicBool::new(false),
            connection: AsyncMutex::new(None),
            status: Mutex::new(Status::default()),
            forwards: AsyncMutex::new(None),
            cancel_dials: tokio::sync::Notify::new(),
            received: AtomicU32::new(0),
            link_received: AtomicU32::new(0),
        });
        shared.watch_relay(&endpoint);
        let accept = tokio::spawn({
            let shared = shared.clone();
            let path = path.clone();
            async move {
                loop {
                    match listener.accept().await {
                        Ok((socket, _)) => {
                            let _ = socket.set_nodelay(true);
                            tokio::spawn(forward(socket, shared.clone(), path.clone()));
                        }
                        // The system took the port back (iOS does, from an
                        // app suspended with its screen locked): nothing
                        // reaches the tunnel any more, so it says so, and
                        // the app opens a new one.
                        Err(error) => {
                            shared.failed(&format!("The tunnel's local port closed: {error}"), Failure::Other);
                            break;
                        }
                    }
                }
            }
        });
        Ok(Self { port, path: path.to_string(), shared, accept })
    }

    /// The loopback port the app connects to: ws://127.0.0.1:<port><path>.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The path the app asks for there; nothing else is forwarded.
    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn status(&self) -> Status {
        let mut status = self.shared.status.lock().unwrap().clone();
        status.received = self.shared.received.load(Ordering::Relaxed);
        status.link_received = self.shared.link_received.load(Ordering::Relaxed);
        status.dial_timeout_ms = u32::try_from(self.shared.dial_timeout().as_millis()).unwrap_or(u32::MAX);
        status
    }

    /// After the app was suspended, or the network changed under it: a
    /// dial in flight is told to give up (it may be stuck on a dead path,
    /// and sockets would queue behind it), the connection is closed, so
    /// the backend sees this session end now rather than when it times
    /// out, and the endpoint is made afresh. The next socket dials on the
    /// new one, waiting for it if it's still being made. A failure from
    /// before says nothing about now.
    ///
    /// Made afresh because telling iroh the network changed isn't enough:
    /// back on the same network (a phone unlocked on the same Wi-Fi), it
    /// finds nothing changed and keeps its sockets and its relay
    /// connection, which a suspended app's may have lost, so every dial
    /// waits out its timeout (netwatch doesn't look for sleep on phones).
    /// The old endpoint is closed before the new one is made, so the relay
    /// never sees two with this device's key. One made, or that made a
    /// connection, less than [REBUILD_AFTER] ago is only told the network
    /// changed, and that wait doubles with each rebuild no connection
    /// followed ([rebuild_age]). A reconnect called while another is under
    /// way joins it: it returns when that one ends, and does nothing of
    /// its own, so retries can't stack rebuilds or undo what it made.
    pub async fn reconnect(&self) {
        let shared = &self.shared;
        // One at a time: a reconnect while another is under way joins it,
        // returning when it ends, and leaves what it made alone.
        let Ok(_reconnecting) = shared.reconnecting.try_lock() else {
            drop(shared.reconnecting.lock().await);
            return;
        };
        shared.cancel_dials.notify_waiters();
        {
            // Free at once, or as soon as the cancelled dial lets go.
            let mut slot = shared.connection.lock().await;
            if let Some(connection) = slot.take() {
                connection.close(0u32.into(), b"");
            }
        }
        if let Some(connection) = shared.forwards.lock().await.take() {
            connection.close(0u32.into(), b"");
        }
        shared.reset_status();
        let mut slot = shared.endpoint.lock().await;
        if slot.closed {
            return;
        }
        let young = shared.quiet(&slot) < rebuild_age(shared.rebuild_after, shared.unanswered.load(Ordering::Relaxed));
        let Some(make) = shared.make.clone().filter(|_| !young || slot.endpoint.is_none()) else {
            if let Some(endpoint) = slot.endpoint.clone() {
                drop(slot);
                let _ = tokio::time::timeout(NETWORK_CHANGE_TIMEOUT, endpoint.network_change()).await;
            }
            return;
        };
        // On failure, left for the next dial to make; until then, nothing
        // failed.
        if shared.replace_endpoint(&mut slot, &make).await.is_ok() {
            shared.unanswered.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Stops listening and closes the connection and the endpoint.
    ///
    /// Bounded: the endpoint has [CLOSE_TIMEOUT] to close, then it's
    /// dropped, which aborts it. Either way, when this returns no endpoint
    /// with this device's key is left of this tunnel, so the next may open
    /// at once.
    pub async fn close(&self) {
        self.accept.abort();
        self.shared.stop_relay_watch();
        // A dial holds the endpoint: let go of it.
        self.shared.cancel_dials.notify_waiters();
        if let Some(connection) = self.shared.connection.lock().await.take() {
            connection.close(0u32.into(), b"closed");
        }
        if let Some(connection) = self.shared.forwards.lock().await.take() {
            connection.close(0u32.into(), b"closed");
        }
        let endpoint = {
            let mut slot = self.shared.endpoint.lock().await;
            slot.closed = true;
            slot.endpoint.take()
        };
        if let Some(endpoint) = endpoint {
            close_endpoint(endpoint).await;
        }
    }
}

impl Tunnel {
    /// What localhost forwards dial with.
    pub(crate) fn dialer(&self) -> Dialer {
        Dialer(self.shared.clone())
    }
}

/// A tunnel's side for localhost forwards (`crate::forward`).
#[derive(Clone)]
pub(crate) struct Dialer(Arc<Shared>);

impl Dialer {
    /// The open forwarding connection, or a new one.
    pub(crate) async fn forward_connection(&self) -> Result<Connection, crate::forward::ForwardingFailure> {
        use crate::forward::{ALPN_FORWARD, ForwardingFailure};
        let mut slot = self.0.forwards.lock().await;
        if let Some(open) = slot.as_ref()
            && open.close_reason().is_none()
        {
            return Ok(open.clone());
        }
        let endpoint = self.0.endpoint().await.map_err(|error| ForwardingFailure::Failed(error.to_string()))?;
        let dial = endpoint.connect(self.0.target(), ALPN_FORWARD);
        let within = self.0.dial_timeout();
        let dialled = tokio::select! {
            dialled = tokio::time::timeout(within, dial) => dialled,
            // reconnect or close: let go of this endpoint.
            _ = self.0.cancel_dials.notified() => {
                return Err(ForwardingFailure::Failed("The tunnel is reconnecting.".into()));
            }
        };
        drop(endpoint);
        let connection = match dialled {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) if refused_alpn(&error) => return Err(ForwardingFailure::Unsupported),
            Ok(Err(error)) => return Err(ForwardingFailure::Failed(explain_connect(&error).0)),
            Err(_) => {
                self.0.dial_timed_out();
                return Err(ForwardingFailure::Failed(format!("Nothing answered within {} seconds.", within.as_secs())));
            }
        };
        self.0.dial_made();
        self.0.connected();
        *slot = Some(connection.clone());
        Ok(connection)
    }
}

/// The backend doesn't speak the ALPN asked for (TLS's
/// no_application_protocol alert): it's from before it.
fn refused_alpn(error: &ConnectError) -> bool {
    let said = format!("{error:#}").to_lowercase();
    ["no_application_protocol", "no application protocol", "error 120", "doesn't support any known protocol"]
        .iter()
        .any(|sign| said.contains(sign))
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.accept.abort();
        self.shared.stop_relay_watch();
    }
}

/// One app socket: once it has asked for the tunnel's [path], a new stream
/// on the connection (dialling it if needed), bytes copied both ways until
/// either side ends, its request line asking for `/` as before. A socket
/// that asks for anything else is closed without a word, before anything
/// is dialled; one the tunnel can't carry is closed at once, and the
/// status says why.
async fn forward(mut socket: TcpStream, shared: Arc<Shared>, path: Arc<str>) {
    let Ok(Ok(first)) = tokio::time::timeout(REQUEST_TIMEOUT, admitted(&mut socket, &path)).await else {
        return;
    };
    let Some(connection) = connection(&shared).await else {
        return;
    };
    let (mut send, receive) = match connection.open_bi().await {
        Ok(streams) => streams,
        Err(error) => {
            shared.failed(&error.to_string(), Failure::Other);
            return;
        }
    };
    let (mut read, mut write) = socket.into_split();
    if connection.alpn() == ALPN_COMPRESSED {
        // copy flushes whenever the socket has nothing more to give, so each
        // message goes at once.
        let up = async {
            let mut send = ZstdEncoder::new(send);
            // Flushed now: what follows may not come until the backend answers.
            if send.write_all(&first).await.is_err() || send.flush().await.is_err() {
                return;
            }
            let _ = copy(&mut read, &mut send).await;
            let _ = send.shutdown().await;
        };
        let down = async {
            let mut receive = ZstdDecoder::new(BufReader::new(Counted { inner: receive, shared: shared.clone() }));
            let _ = copy(&mut receive, &mut write).await;
            let _ = write.shutdown().await;
        };
        tokio::join!(up, down);
        return;
    }
    let up = async {
        if send.write_all(&first).await.is_ok() {
            let _ = copy(&mut read, &mut send).await;
        }
        let _ = send.finish();
    };
    let down = async {
        let _ = copy(&mut Counted { inner: receive, shared: shared.clone() }, &mut write).await;
        let _ = write.shutdown().await;
    };
    tokio::join!(up, down);
}

/// [inner], adding what's read from it to [Status::received].
struct Counted<R> {
    inner: R,
    shared: Arc<Shared>,
}

impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Counted<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        let read = buf.filled().len() - before;
        if read > 0 {
            // Wrapping: only whether it moved matters.
            self.shared.received.fetch_add(read as u32, Ordering::Relaxed);
        }
        polled
    }
}

/// Reads [socket]'s request line and checks it asks for [path]: then what
/// was read, the line asking for `/` instead (what the backend expects),
/// followed by whatever came after it.
async fn admitted(socket: &mut TcpStream, path: &str) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let refused = || std::io::Error::new(std::io::ErrorKind::PermissionDenied, "not the tunnel's path");
    let mut read = Vec::with_capacity(512);
    let end = loop {
        if let Some(end) = read.windows(2).position(|pair| pair == b"\r\n") {
            break end;
        }
        if read.len() > REQUEST_LINE_MAX {
            return Err(refused());
        }
        let mut chunk = [0u8; 512];
        // Never more than a line's worth and its end past the cap.
        let room = (REQUEST_LINE_MAX + 2 - read.len()).min(chunk.len());
        let n = socket.read(&mut chunk[..room]).await?;
        if n == 0 {
            return Err(refused());
        }
        read.extend_from_slice(&chunk[..n]);
    };
    if end > REQUEST_LINE_MAX {
        return Err(refused());
    }
    let line = &read[..end];
    let mut parts = line.split(|byte| *byte == b' ');
    let (Some(b"GET"), Some(target), Some(version), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(refused());
    };
    if !same(target, path.as_bytes()) {
        return Err(refused());
    }
    let mut first = Vec::with_capacity(read.len());
    first.extend_from_slice(b"GET / ");
    first.extend_from_slice(version);
    first.extend_from_slice(&read[end..]);
    Ok(first)
}

/// Whether [a] and [b] are the same, taking as long whatever differs.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

/// Dials the backend offering [ALPN_COMPRESSED], then [ALPN].
async fn dial(shared: &Shared, endpoint: &Endpoint) -> Result<Connection, ConnectError> {
    let options = ConnectOptions::new().with_additional_alpns(vec![ALPN.to_vec()]);
    Ok(endpoint.connect_with_opts(shared.target(), ALPN_COMPRESSED, options).await?.await?)
}

/// How a dial ended.
enum Dialled {
    Made(Connection),
    Failed(ConnectError),
    /// Nothing within the time it had.
    TimedOut(Duration),
    /// `reconnect` or `close` told it to give up.
    Cancelled,
}

/// [dial] within the time it has now ([DIAL_TIMEOUTS]), given up when
/// `reconnect` says so: the dial may be stuck on a path that's gone. One
/// that times out gives the next longer; one made, the first again.
async fn dial_within(shared: &Shared, endpoint: &Endpoint) -> Dialled {
    let within = shared.dial_timeout();
    tokio::select! {
        dialled = tokio::time::timeout(within, dial(shared, endpoint)) => match dialled {
            Ok(Ok(connection)) => {
                shared.dial_made();
                Dialled::Made(connection)
            }
            Ok(Err(error)) => Dialled::Failed(error),
            Err(_) => {
                shared.dial_timed_out();
                Dialled::TimedOut(within)
            }
        },
        _ = shared.cancel_dials.notified() => Dialled::Cancelled,
    }
}

/// Whether [connection], made on a direct path, stalled: the backend
/// hasn't confirmed it within [CONFIRM_WITHIN] ([CONFIRM_WITHIN] says why
/// that happens). None when `reconnect` cut the wait short.
async fn stalled(shared: &Shared, connection: &Connection) -> Option<bool> {
    if path_of(connection) == Path::Relayed {
        return Some(false);
    }
    let confirmed = async {
        let until = Instant::now() + CONFIRM_WITHIN;
        loop {
            // A backend that closed it (turning the device away, say) has
            // answered: that's no stall, and the close says why.
            if connection.stats().frame_rx.handshake_done > 0 || connection.close_reason().is_some() {
                return false;
            }
            if Instant::now() >= until {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        stalled = confirmed => Some(stalled),
        _ = shared.cancel_dials.notified() => None,
    }
}

/// The open connection, or a new one; None when it can't be made (the
/// status says why).
async fn connection(shared: &Arc<Shared>) -> Option<Connection> {
    let mut slot = shared.connection.lock().await;
    if let Some(open) = slot.as_ref()
        && open.close_reason().is_none()
    {
        return Some(open.clone());
    }
    // Waits while reconnect makes it afresh.
    let endpoint = match shared.endpoint().await {
        Ok(endpoint) => endpoint,
        Err(error) => {
            shared.failed(&format!("This device couldn't start its side of the tunnel: {error}"), Failure::Other);
            return None;
        }
    };
    shared.status.lock().unwrap().phase = Phase::Dialing;
    let relay_first = shared.relay_first.load(Ordering::Relaxed);
    let mut dialled = dial_within(shared, &endpoint).await;
    if let Dialled::Made(connection) = &dialled
        && !relay_first
        && shared.backend.relay_urls().next().is_some()
    {
        match stalled(shared, connection).await {
            Some(false) => {}
            Some(true) => {
                // Through the relay only, on an endpoint made afresh: this
                // one would send the next dial's first packet down the
                // stalled path again, since iroh keeps every address it
                // was given for the backend.
                connection.close(0u32.into(), b"stalled");
                shared.relay_first.store(true, Ordering::Relaxed);
                drop(endpoint);
                dialled = match shared.remake_endpoint().await {
                    Ok(endpoint) => dial_within(shared, &endpoint).await,
                    Err(error) => {
                        shared.failed(&format!("This device couldn't start its side of the tunnel: {error}"), Failure::Other);
                        *slot = None;
                        return None;
                    }
                };
            }
            None => dialled = Dialled::Cancelled,
        }
    }
    if shared.relay_first.load(Ordering::Relaxed) && !matches!(dialled, Dialled::Made(_) | Dialled::Cancelled) {
        shared.relay_first.store(false, Ordering::Relaxed);
    }
    let connection = match dialled {
        Dialled::Made(connection) => connection,
        // `reconnect`: give up quietly; the next socket dials afresh on
        // what the network is now.
        Dialled::Cancelled => {
            let mut status = shared.status.lock().unwrap();
            status.phase = Phase::Idle;
            status.path = Path::None;
            status.reach = Reach::None;
            *slot = None;
            return None;
        }
        Dialled::Failed(error) => {
            let (said, failure) = explain_connect(&error);
            shared.failed(&said, failure);
            *slot = None;
            return None;
        }
        Dialled::TimedOut(within) => {
            let relay = shared.status.lock().unwrap().relay_connected;
            if relay {
                let said = format!("Nothing answered within {} seconds: the machine may be off, asleep or offline.", within.as_secs());
                shared.failed(&said, Failure::Unreachable);
            } else {
                shared.failed("This device can't reach the relay or the machine: check its network.", Failure::Offline);
            }
            *slot = None;
            return None;
        }
    };
    {
        let mut status = shared.status.lock().unwrap();
        status.phase = Phase::Connected;
        status.error.clear();
        status.failure = Failure::None;
        status.path = path_of(&connection);
        status.reach = reach_of(&connection);
    }
    shared.connected();
    tokio::spawn(follow(connection.clone(), shared.clone()));
    *slot = Some(connection.clone());
    Some(connection)
}

/// Keeps the status's path up to date while [connection] lives, and says
/// why it closed.
async fn follow(connection: Connection, shared: Arc<Shared>) {
    let update = {
        let connection = connection.clone();
        let shared = shared.clone();
        async move {
            loop {
                let (path, reach) = (path_of(&connection), reach_of(&connection));
                {
                    let mut status = shared.status.lock().unwrap();
                    status.path = path;
                    status.reach = reach;
                }
                // Wrapping: only whether it moved matters.
                shared.link_received.store(connection.stats().udp_rx.bytes as u32, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    };
    let closed = connection.closed();
    tokio::select! {
        error = closed => {
            let (said, failure) = explain_closed(&error);
            let mut status = shared.status.lock().unwrap();
            status.path = Path::None;
            status.reach = Reach::None;
            if failure == Failure::None {
                status.phase = Phase::Idle;
            } else {
                status.phase = Phase::Failed;
                status.error = said;
                status.failure = failure;
            }
        }
        _ = update => {}
    }
}

/// What the selected path crosses, when it's direct.
fn reach_of(connection: &Connection) -> Reach {
    let paths = connection.paths();
    match paths.iter().find(|path| path.is_selected()) {
        Some(path) => match path.remote_addr() {
            iroh::TransportAddr::Ip(addr) => Reach::of(addr),
            _ => Reach::None,
        },
        None => Reach::None,
    }
}

fn path_of(connection: &Connection) -> Path {
    let paths = connection.paths();
    let selected = paths.iter().find(|path| path.is_selected());
    match selected {
        Some(path) if path.is_relay() => Path::Relayed,
        Some(_) => Path::Direct,
        None => Path::None,
    }
}

impl Shared {
    /// The current endpoint, made now if there's none (making it failed
    /// before); waits while [Tunnel::reconnect] makes it.
    async fn endpoint(self: &Arc<Self>) -> anyhow::Result<Endpoint> {
        let mut slot = self.endpoint.lock().await;
        if let Some(endpoint) = &slot.endpoint {
            return Ok(endpoint.clone());
        }
        let make = self.make.as_ref().filter(|_| !slot.closed).ok_or_else(|| anyhow::anyhow!("the tunnel is closed"))?;
        let endpoint = make_within(make).await?;
        self.watch_relay(&endpoint);
        slot.endpoint = Some(endpoint.clone());
        slot.made = Instant::now();
        Ok(endpoint)
    }

    /// How long the next dial has: [DIAL_TIMEOUTS], by how many to this
    /// backend have timed out in a row.
    fn dial_timeout(&self) -> Duration {
        let timed_out = DIALS_TIMED_OUT.lock().unwrap().iter().find(|(id, _)| *id == self.backend.id).map_or(0, |(_, n)| *n);
        DIAL_TIMEOUTS[timed_out.min(DIAL_TIMEOUTS.len() - 1)]
    }

    fn dial_timed_out(&self) {
        let mut all = DIALS_TIMED_OUT.lock().unwrap();
        match all.iter_mut().find(|(id, _)| *id == self.backend.id) {
            Some((_, n)) => *n = (*n + 1).min(DIAL_TIMEOUTS.len() - 1),
            None => all.push((self.backend.id, 1)),
        }
    }

    fn dial_made(&self) {
        DIALS_TIMED_OUT.lock().unwrap().retain(|(id, _)| *id != self.backend.id);
    }

    /// Where dials go: the backend's pairing code, or only its relay once
    /// a direct path stalled ([Shared::relay_first]).
    fn target(&self) -> EndpointAddr {
        if self.relay_first.load(Ordering::Relaxed) {
            EndpointAddr::from_parts(self.backend.id, self.backend.relay_urls().cloned().map(iroh::TransportAddr::Relay))
        } else {
            self.backend.clone()
        }
    }

    /// The endpoint made afresh, for a dial that mustn't meet what the
    /// old one knew; the same one when it can't be made afresh.
    async fn remake_endpoint(self: &Arc<Self>) -> anyhow::Result<Endpoint> {
        let mut slot = self.endpoint.lock().await;
        let make = self.make.as_ref().filter(|_| !slot.closed).cloned();
        match make {
            Some(make) => self.replace_endpoint(&mut slot, &make).await,
            None => slot.endpoint.clone().ok_or_else(|| anyhow::anyhow!("the tunnel is closed")),
        }
    }

    /// Closes [slot]'s endpoint, then makes another in its place: never
    /// two with this device's key at once, for the relay. On failure the
    /// slot is left empty, for the next dial to make.
    async fn replace_endpoint(self: &Arc<Self>, slot: &mut Slot, make: &MakeEndpoint) -> anyhow::Result<Endpoint> {
        if let Some(old) = slot.endpoint.take() {
            self.stop_relay_watch();
            close_endpoint(old).await;
        }
        self.status.lock().unwrap().relay_connected = false;
        let endpoint = make_within(make).await?;
        self.watch_relay(&endpoint);
        slot.endpoint = Some(endpoint.clone());
        slot.made = Instant::now();
        Ok(endpoint)
    }

    /// Follows whether [endpoint] reaches its home relay, in place of the
    /// one followed before.
    fn watch_relay(self: &Arc<Self>, endpoint: &Endpoint) {
        let shared = Arc::downgrade(self);
        let mut relays = endpoint.home_relay_status();
        {
            let mut status = self.status.lock().unwrap();
            status.relay_up_ms = 0;
            status.relay_trouble = RelayTrouble::None;
        }
        let watch = tokio::spawn(async move {
            // Since the endpoint was made, or the relay was last lost.
            let mut down_since = Some(Instant::now());
            // Since no home relay has been chosen: the endpoint is still
            // measuring, until UNCHOSEN_AFTER says it found none.
            let mut unchosen_since: Option<Instant> = None;
            loop {
                let now = relays.get();
                let connected = now.iter().any(|relay| relay.is_connected());
                // When to look again without an update: once still nothing
                // chosen counts as trouble.
                let mut look_again = None;
                if !now.is_empty() {
                    unchosen_since = None;
                }
                let trouble = if connected {
                    RelayTrouble::None
                } else if now.is_empty() {
                    let since = *unchosen_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= UNCHOSEN_AFTER {
                        RelayTrouble::Unchosen
                    } else {
                        look_again = Some(since + UNCHOSEN_AFTER);
                        RelayTrouble::None
                    }
                } else {
                    now.iter().find_map(|relay| relay.last_error()).map(|error| RelayTrouble::of(&format!("{error:#}"))).unwrap_or_default()
                };
                let Some(shared) = shared.upgrade() else { break };
                {
                    let mut status = shared.status.lock().unwrap();
                    status.relay_connected = connected;
                    status.relay_trouble = trouble;
                    if connected {
                        if let Some(since) = down_since.take() {
                            status.relay_up_ms = u32::try_from(since.elapsed().as_millis()).unwrap_or(u32::MAX).max(1);
                        }
                    } else {
                        status.relay_up_ms = 0;
                        down_since.get_or_insert_with(Instant::now);
                    }
                }
                drop(shared);
                let updated = match look_again {
                    Some(at) => tokio::select! {
                        updated = relays.updated() => updated.is_ok(),
                        () = tokio::time::sleep_until(at.into()) => true,
                    },
                    None => relays.updated().await.is_ok(),
                };
                if !updated {
                    break;
                }
            }
        });
        if let Some(old) = self.relay_watch.lock().unwrap().replace(watch) {
            old.abort();
        }
    }

    fn stop_relay_watch(&self) {
        if let Some(watch) = self.relay_watch.lock().unwrap().take() {
            watch.abort();
        }
    }

    /// How long since [slot]'s endpoint was made or last made a
    /// connection, whichever is later.
    fn quiet(&self, slot: &Slot) -> Duration {
        match *self.connected_at.lock().unwrap() {
            Some(at) => slot.made.elapsed().min(at.elapsed()),
            None => slot.made.elapsed(),
        }
    }

    /// A connection was made: rebuilds start from the first wait again,
    /// and none is made for that long.
    fn connected(&self) {
        self.unanswered.store(0, Ordering::Relaxed);
        *self.connected_at.lock().unwrap() = Some(Instant::now());
    }

    /// Nothing failed, nothing's connected: what reconnect starts from.
    fn reset_status(&self) {
        let mut status = self.status.lock().unwrap();
        status.phase = Phase::Idle;
        status.path = Path::None;
        status.reach = Reach::None;
        status.error.clear();
        status.failure = Failure::None;
    }

    fn failed(&self, said: &str, failure: Failure) {
        let mut status = self.status.lock().unwrap();
        status.phase = Phase::Failed;
        status.path = Path::None;
        status.reach = Reach::None;
        status.error = said.to_owned();
        status.failure = failure;
    }
}

/// What a failed dial means for the person.
fn explain_connect(error: &iroh::endpoint::ConnectError) -> (String, Failure) {
    use iroh::endpoint::{ConnectError, ConnectWithOptsError, ConnectingError};
    match error {
        ConnectError::Connect { source, .. } => match source {
            ConnectWithOptsError::NoAddress { .. } => (
                "Couldn't find the machine: it hasn't been online since its pairing code was made, \
                 or this device can't reach the relay."
                    .to_owned(),
                Failure::Unreachable,
            ),
            other => (other.to_string(), Failure::Other),
        },
        ConnectError::Connecting { source, .. } => match source {
            ConnectingError::ConnectionError { source, .. } => explain_closed(source),
            other => (other.to_string(), Failure::Other),
        },
        ConnectError::Connection { source, .. } => explain_closed(source),
        other => (other.to_string(), Failure::Other),
    }
}

/// What a connection's end means; [Failure::None] for an orderly close.
fn explain_closed(error: &ConnectionError) -> (String, Failure) {
    match error {
        ConnectionError::LocallyClosed => (String::new(), Failure::None),
        ConnectionError::ApplicationClosed(close) => {
            let reason = String::from_utf8_lossy(&close.reason).trim().to_owned();
            if u64::from(close.error_code) == 0 && reason.is_empty() {
                (String::new(), Failure::None)
            } else if reason.is_empty() {
                ("The machine turned this device away.".to_owned(), Failure::Refused)
            } else {
                (format!("The machine turned this device away: {reason}"), Failure::Refused)
            }
        }
        ConnectionError::ConnectionClosed(close) => (format!("The machine closed the connection: {close}"), Failure::Refused),
        ConnectionError::TimedOut => {
            ("The connection to the machine timed out: it may be off, asleep or offline.".to_owned(), Failure::Unreachable)
        }
        other => (other.to_string(), Failure::Other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::RelayMode;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// On the real network (#388): a device endpoint homes on Claktory's
    /// relay, and the status says how long that took; one whose relay
    /// can't be reached says none was chosen. `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "needs the network and relay.claktory.com"]
    async fn the_status_says_how_long_the_relay_took_or_why_it_cant_be_reached() {
        let backend = echo_backend().await;
        let make: MakeEndpoint = Arc::new(|| Box::pin(device_endpoint(SecretKey::generate())));
        let tunnel = Tunnel::open_with(make, REBUILD_AFTER, backend.addr()).await.unwrap();
        let begun = Instant::now();
        while !tunnel.status().relay_connected && begun.elapsed() < Duration::from_secs(15) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let status = tunnel.status();
        assert!(status.relay_connected, "{status:?}");
        assert!(status.relay_up_ms > 0 && u128::from(status.relay_up_ms) <= begun.elapsed().as_millis() + 1000, "{status:?}");
        assert_eq!(status.relay_trouble, RelayTrouble::None);
        eprintln!("home relay up after {} ms", status.relay_up_ms);
        tunnel.close().await;

        let nowhere: MakeEndpoint = Arc::new(|| {
            Box::pin(async {
                let relay: RelayUrl = "https://relay.claktory.invalid./".parse()?;
                Ok(Endpoint::builder(presets::N0)
                    .relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from(relay)))
                    .secret_key(SecretKey::generate())
                    .bind()
                    .await?)
            })
        });
        let tunnel = Tunnel::open_with(nowhere, REBUILD_AFTER, backend.addr()).await.unwrap();
        let begun = Instant::now();
        while tunnel.status().relay_trouble == RelayTrouble::None && begun.elapsed() < Duration::from_secs(10) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let status = tunnel.status();
        assert!(!status.relay_connected);
        assert_eq!(status.relay_up_ms, 0);
        // Never reached, so never chosen as its home: iroh says no more.
        assert_eq!(status.relay_trouble, RelayTrouble::Unchosen, "{status:?}");
        tunnel.close().await;
    }

    /// A direct path's address says what it crosses, in a word (Polpo's
    /// study §4): never the address itself.
    #[test]
    fn a_direct_path_says_what_it_crosses() {
        let reach = |addr: &str| Reach::of(&addr.parse().unwrap());
        assert_eq!(reach("127.0.0.1:1"), Reach::Local);
        assert_eq!(reach("[::1]:1"), Reach::Local);
        assert_eq!(reach("192.168.1.20:7"), Reach::Lan);
        assert_eq!(reach("10.0.0.2:7"), Reach::Lan);
        assert_eq!(reach("172.20.1.1:7"), Reach::Lan);
        assert_eq!(reach("169.254.3.4:7"), Reach::Lan);
        assert_eq!(reach("[fe80::1]:7"), Reach::Lan);
        assert_eq!(reach("[fd12::1]:7"), Reach::Lan);
        assert_eq!(reach("100.101.2.3:7"), Reach::Tailnet);
        assert_eq!(reach("[fd7a:115c:a1e0::5]:7"), Reach::Tailnet);
        assert_eq!(reach("[::ffff:100.64.0.1]:7"), Reach::Tailnet);
        assert_eq!(reach("100.128.0.1:7"), Reach::Public);
        assert_eq!(reach("49.12.7.63:7"), Reach::Public);
        assert_eq!(reach("[2a01:4f8::1]:7"), Reach::Public);
    }

    /// While the endpoint is still measuring the network, no home relay
    /// chosen is no trouble yet; once it has had UNCHOSEN_AFTER and chose
    /// none (its only relay refuses), it is (Zucchero's #388 finding: every
    /// launch noted err=unchosen before its relay came up).
    #[tokio::test]
    async fn no_relay_chosen_is_trouble_only_once_the_first_measurement_had_its_time() {
        let backend = echo_backend().await;
        let nowhere: MakeEndpoint = Arc::new(|| {
            Box::pin(async {
                let relay: RelayUrl = "http://127.0.0.1:9/".parse()?;
                Ok(Endpoint::builder(presets::Minimal)
                    .relay_mode(iroh::RelayMode::Custom(iroh::RelayMap::from(relay)))
                    .secret_key(SecretKey::generate())
                    .bind()
                    .await?)
            })
        });
        let tunnel = Tunnel::open_with(nowhere, REBUILD_AFTER, backend.addr()).await.unwrap();
        let begun = Instant::now();
        while begun.elapsed() < UNCHOSEN_AFTER - Duration::from_millis(500) {
            let status = tunnel.status();
            assert_eq!(status.relay_trouble, RelayTrouble::None, "at {:?}: {status:?}", begun.elapsed());
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        while tunnel.status().relay_trouble == RelayTrouble::None && begun.elapsed() < UNCHOSEN_AFTER + Duration::from_secs(3) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let status = tunnel.status();
        assert!(!status.relay_connected);
        assert_eq!(status.relay_trouble, RelayTrouble::Unchosen, "{status:?}");
        assert!(begun.elapsed() < UNCHOSEN_AFTER + Duration::from_secs(1), "{:?}", begun.elapsed());
        tunnel.close().await;
    }

    #[test]
    fn a_relay_error_is_kept_only_as_its_class() {
        assert_eq!(RelayTrouble::of("unable to connect: dns lookup failed: no record found"), RelayTrouble::Dns);
        assert_eq!(RelayTrouble::of("timeout (>10s) trying to establish a connection"), RelayTrouble::Timeout);
        assert_eq!(RelayTrouble::of("unable to connect: tls connection failed: invalid peer certificate"), RelayTrouble::Tls);
        assert_eq!(RelayTrouble::of("unable to connect: Connection refused (os error 61)"), RelayTrouble::Network);
        assert_eq!(RelayTrouble::of("unable to connect: Network is unreachable (os error 51)"), RelayTrouble::Network);
        assert_eq!(RelayTrouble::of("Unexpected status during upgrade: 502"), RelayTrouble::Other);
    }

    #[test]
    fn the_device_homes_on_claktorys_relay_only() {
        let urls: Vec<RelayUrl> = device_relays().relay_map().urls();
        assert_eq!(urls.iter().map(ToString::to_string).collect::<Vec<_>>(), vec![CLAKTORY_RELAY.to_owned()]);
    }

    /// A backend stand-in on this machine, no relays: echoes every stream.
    async fn echo_backend() -> Endpoint {
        echo_backend_with(&[ALPN]).await
    }

    /// An echoing backend with these ALPNs: on `claktory/2` it decompresses
    /// what comes and compresses what it sends back, as factoryd does.
    async fn echo_backend_with(alpns: &[&[u8]]) -> Endpoint {
        let alpns = alpns.iter().map(|alpn| alpn.to_vec()).collect();
        let endpoint = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).alpns(alpns).bind().await.unwrap();
        serve_echo(&endpoint);
        endpoint
    }

    /// Echoes on [endpoint] as [echo_backend_with] says.
    fn serve_echo(endpoint: &Endpoint) {
        let server = endpoint.clone();
        tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                tokio::spawn(async move {
                    let connection = incoming.await.unwrap();
                    let compressed = connection.alpn() == ALPN_COMPRESSED;
                    while let Ok((mut send, mut receive)) = connection.accept_bi().await {
                        tokio::spawn(async move {
                            if compressed {
                                let mut decoder = ZstdDecoder::new(BufReader::new(receive));
                                let mut encoder = ZstdEncoder::new(send);
                                // Message by message, flushed, as a WebSocket sends.
                                let mut buffer = [0u8; 4096];
                                loop {
                                    match decoder.read(&mut buffer).await {
                                        Ok(0) | Err(_) => break,
                                        Ok(read) => {
                                            encoder.write_all(&buffer[..read]).await.unwrap();
                                            encoder.flush().await.unwrap();
                                        }
                                    }
                                }
                                let _ = encoder.shutdown().await;
                            } else {
                                let _ = copy(&mut receive, &mut send).await;
                                let _ = send.finish();
                            }
                        });
                    }
                });
            }
        });
    }

    async fn local_device() -> Endpoint {
        Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .secret_key(parse_key(&generate_key()).unwrap())
            .bind()
            .await
            .unwrap()
    }

    /// A socket that asked for the tunnel's path, as the app's WebSocket
    /// does: the backend gets (and the echo returns) `GET / HTTP/1.1`.
    async fn ask(tunnel: &Tunnel) -> TcpStream {
        let mut socket = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
        socket.write_all(format!("GET {} HTTP/1.1\r\n", tunnel.path()).as_bytes()).await.unwrap();
        let mut got = [0u8; 16];
        tokio::time::timeout(Duration::from_secs(5), socket.read_exact(&mut got)).await.expect("in time").unwrap();
        assert_eq!(&got, b"GET / HTTP/1.1\r\n");
        socket
    }

    #[tokio::test]
    async fn sockets_become_streams_on_one_connection() {
        let backend = echo_backend().await;
        let tunnel = Tunnel::open(local_device().await, backend.addr()).await.unwrap();
        assert_eq!(tunnel.status().phase, Phase::Idle, "nothing dials before the app connects");

        // Two sockets at once, each its own stream, bytes both ways.
        let mut first = ask(&tunnel).await;
        let mut second = ask(&tunnel).await;
        first.write_all(b"first").await.unwrap();
        second.write_all(b"second").await.unwrap();
        let mut got = [0u8; 5];
        first.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"first");
        let mut got = [0u8; 6];
        second.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"second");

        let status = tunnel.status();
        assert_eq!(status.phase, Phase::Connected);
        assert_eq!(status.path, Path::Direct, "same machine, no relays");
        assert!(matches!(status.reach, Reach::Local | Reach::Lan), "{:?}", status.reach);
        assert!(status.error.is_empty());

        // A closed socket ends its stream; the echo ends it back.
        first.shutdown().await.unwrap();
        let mut rest = Vec::new();
        first.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
        tunnel.close().await;
    }

    #[tokio::test]
    async fn only_a_socket_asking_for_the_tunnels_path_is_forwarded() {
        let backend = echo_backend().await;
        let tunnel = Tunnel::open(local_device().await, backend.addr()).await.unwrap();
        assert!(tunnel.path().len() == 65 && tunnel.path().starts_with('/'), "{}", tunnel.path());
        let other = Tunnel::open(local_device().await, backend.addr()).await.unwrap();
        assert_ne!(tunnel.path(), other.path(), "every tunnel has its own");
        other.close().await;

        let wrong = format!("GET /{} HTTP/1.1\r\nHost: x\r\n\r\n", "0".repeat(64));
        let line = "GET /".to_owned() + &"a".repeat(2000);
        // The right path, but a line past the cap, its end just beyond it.
        let long = format!("GET {} HTTP/1.1{}\r\n", tunnel.path(), "1".repeat(REQUEST_LINE_MAX + 100 - 74));
        let refused: [&[u8]; 7] = [
            long.as_bytes(),
            b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            wrong.as_bytes(),
            b"POST /x HTTP/1.1\r\n",
            b"hello\r\n",
            // No line's end within the cap.
            line.as_bytes(),
            // Says nothing: dropped once its time is up.
            b"",
        ];
        for asked in refused {
            let mut socket = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
            let _ = socket.write_all(asked).await;
            let mut rest = Vec::new();
            let read = tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut rest)).await.expect("closed in time");
            assert!(read.is_err() || rest.is_empty(), "{:?} got {:?}", String::from_utf8_lossy(asked), rest);
        }
        // Nothing was dialled for any of them.
        assert_eq!(tunnel.status().phase, Phase::Idle);
        assert!(tunnel.shared.connection.lock().await.is_none());

        // The path's own case and a request line in pieces still pass.
        let mut socket = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
        let request = format!("GET {} HTTP/1.1\r\nUpgrade: websocket\r\n\r\n", tunnel.path());
        for piece in request.as_bytes().chunks(7) {
            socket.write_all(piece).await.unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let expected = b"GET / HTTP/1.1\r\nUpgrade: websocket\r\n\r\n";
        let mut got = vec![0u8; expected.len()];
        tokio::time::timeout(Duration::from_secs(5), socket.read_exact(&mut got)).await.expect("in time").unwrap();
        assert_eq!(got, expected);
        let upper = format!("GET {} HTTP/1.1\r\n", tunnel.path().to_uppercase());
        let mut socket = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
        socket.write_all(upper.as_bytes()).await.unwrap();
        let mut rest = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut rest)).await.expect("closed in time");
        assert!(rest.is_empty(), "the path is compared exactly");
        tunnel.close().await;
    }

    #[tokio::test]
    async fn reconnect_dials_afresh_without_a_failure() {
        let backend = echo_backend().await;
        let tunnel = Tunnel::open(local_device().await, backend.addr()).await.unwrap();
        let mut socket = ask(&tunnel).await;
        socket.write_all(b"one").await.unwrap();
        let mut got = [0u8; 3];
        socket.read_exact(&mut got).await.unwrap();
        assert_eq!(tunnel.status().phase, Phase::Connected);

        // As after the app comes back from the background.
        tunnel.reconnect().await;
        let status = tunnel.status();
        assert_eq!(status.phase, Phase::Idle);
        assert_eq!(status.failure, Failure::None, "a resume isn't a failure");
        assert!(status.error.is_empty());

        // The next socket dials a new connection.
        let mut again = ask(&tunnel).await;
        again.write_all(b"two").await.unwrap();
        again.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"two");
        assert_eq!(tunnel.status().phase, Phase::Connected);
        tunnel.close().await;
    }

    #[tokio::test]
    async fn reconnect_aborts_a_dial_in_flight() {
        // A machine that never answers: a bound UDP port nobody reads.
        let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let nobody = SecretKey::generate().public();
        let addr = EndpointAddr::new(nobody).with_ip_addr(dead.local_addr().unwrap());
        let tunnel = Tunnel::open(local_device().await, addr).await.unwrap();

        let mut socket = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
        socket.write_all(format!("GET {} HTTP/1.1\r\n", tunnel.path()).as_bytes()).await.unwrap();
        for _ in 0..100 {
            if tunnel.status().phase == Phase::Dialing {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(tunnel.status().phase, Phase::Dialing);

        // reconnect doesn't wait the dial out: the dial gives up at once…
        let begun = std::time::Instant::now();
        tunnel.reconnect().await;
        assert!(begun.elapsed() < Duration::from_secs(2), "took {:?}", begun.elapsed());
        let status = tunnel.status();
        assert_eq!(status.phase, Phase::Idle, "ready to dial afresh");
        assert_eq!(status.failure, Failure::None);

        // …and the socket that waited on it is closed, not left hanging.
        let mut rest = Vec::new();
        let read = tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut rest)).await.expect("closed in time");
        assert!(read.is_err() || rest.is_empty());
        tunnel.close().await;
    }

    #[tokio::test]
    async fn a_backend_that_turns_the_device_away_says_so() {
        let endpoint = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let server = endpoint.clone();
        tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                if let Ok(connection) = incoming.await {
                    connection.close(1u32.into(), b"this device isn't paired");
                }
            }
        });
        let tunnel = Tunnel::open(local_device().await, endpoint.addr()).await.unwrap();
        let mut socket = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
        socket.write_all(format!("GET {} HTTP/1.1\r\n", tunnel.path()).as_bytes()).await.unwrap();
        let mut rest = Vec::new();
        let _ = socket.read_to_end(&mut rest).await;
        // The close can arrive just after the stream opened.
        for _ in 0..50 {
            if tunnel.status().failure != Failure::None {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let status = tunnel.status();
        assert_eq!(status.phase, Phase::Failed);
        assert_eq!(status.failure, Failure::Refused);
        assert!(status.error.contains("this device isn't paired"), "{}", status.error);
        tunnel.close().await;
    }

    /// Bytes through the tunnel come back whole, one message at a time.
    async fn round_trip(tunnel: &Tunnel) {
        let mut socket = ask(tunnel).await;
        for message in [&b"hello"[..], &[7u8; 100_000][..], b"bye"] {
            socket.write_all(message).await.unwrap();
            let mut got = vec![0u8; message.len()];
            tokio::time::timeout(Duration::from_secs(5), socket.read_exact(&mut got)).await.expect("in time").unwrap();
            assert_eq!(got, message);
        }
        socket.shutdown().await.unwrap();
        let mut rest = Vec::new();
        socket.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
    }

    #[test]
    fn android_sends_packet_by_packet_and_other_devices_keep_irohs_defaults() {
        let offload = |config: iroh::endpoint::QuicTransportConfig| {
            let shown = format!("{config:?}");
            if shown.contains("enable_segmentation_offload: false") {
                false
            } else {
                assert!(shown.contains("enable_segmentation_offload: true"), "{shown}");
                true
            }
        };
        assert!(!offload(transport_for(true)), "Android: no GSO");
        assert!(offload(transport_for(false)), "elsewhere: iroh's default, GSO allowed");
        assert_eq!(
            format!("{:?}", transport_for(false)),
            format!("{:?}", iroh::endpoint::QuicTransportConfig::default()),
            "elsewhere exactly iroh's defaults"
        );
        assert_eq!(offload(device_transport()), !cfg!(target_os = "android"));
    }

    #[tokio::test]
    async fn a_backend_with_claktory_2_gets_it_and_one_without_gets_claktory_1() {
        for (alpns, expected) in [(&[ALPN_COMPRESSED, ALPN][..], ALPN_COMPRESSED), (&[ALPN][..], ALPN)] {
            let backend = echo_backend_with(alpns).await;
            let tunnel = Tunnel::open(local_device().await, backend.addr()).await.unwrap();
            round_trip(&tunnel).await;
            let connection = tunnel.shared.connection.lock().await.clone().expect("connected");
            assert_eq!(connection.alpn(), expected);
            tunnel.close().await;
        }
    }

    /// Makes a device endpoint with one key each time, after [delay], and
    /// keeps every one it made.
    fn maker(delay: Duration) -> (MakeEndpoint, Arc<Mutex<Vec<Endpoint>>>) {
        let key = parse_key(&generate_key()).unwrap();
        let made = Arc::new(Mutex::new(Vec::<Endpoint>::new()));
        let make: MakeEndpoint = Arc::new({
            let made = made.clone();
            move || {
                let (key, made) = (key.clone(), made.clone());
                Box::pin(async move {
                    tokio::time::sleep(delay).await;
                    // Never two at once with this key: every one made
                    // before is closed by now.
                    assert!(made.lock().unwrap().iter().all(Endpoint::is_closed), "an endpoint with this key is still open");
                    let endpoint = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).secret_key(key).bind().await?;
                    made.lock().unwrap().push(endpoint.clone());
                    Ok(endpoint)
                })
            }
        });
        (make, made)
    }

    /// The socket echoes what it's sent.
    async fn echoes(socket: &mut TcpStream, bytes: &[u8]) {
        socket.write_all(bytes).await.unwrap();
        let mut got = vec![0u8; bytes.len()];
        tokio::time::timeout(Duration::from_secs(5), socket.read_exact(&mut got)).await.expect("in time").unwrap();
        assert_eq!(got, bytes);
    }

    /// #214: making the endpoint can hang (a phone back from suspension);
    /// reconnect gives up on it after MAKE_TIMEOUT instead of holding the
    /// endpoint's slot, and every dial behind it, for ever. The next dial
    /// makes one again.
    #[tokio::test]
    async fn a_make_that_hangs_is_given_up_on_and_made_again() {
        let backend = echo_backend().await;
        let key = parse_key(&generate_key()).unwrap();
        let calls = Arc::new(AtomicU32::new(0));
        let make: MakeEndpoint = Arc::new({
            let calls = calls.clone();
            move || {
                let (key, calls) = (key.clone(), calls.clone());
                Box::pin(async move {
                    // The second never finishes.
                    if calls.fetch_add(1, Ordering::SeqCst) == 1 {
                        std::future::pending::<()>().await;
                    }
                    Ok(Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).secret_key(key).bind().await?)
                })
            }
        });
        let tunnel = Tunnel::open_with(make, Duration::ZERO, backend.addr()).await.unwrap();
        let begun = Instant::now();
        tokio::time::timeout(MAKE_TIMEOUT * 3, tunnel.reconnect()).await.expect("reconnect gives up on the stuck make");
        assert!(begun.elapsed() >= MAKE_TIMEOUT, "it waited the make out");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // The next socket makes it again, and connects.
        let mut socket = ask(&tunnel).await;
        echoes(&mut socket, b"made again").await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        tunnel.close().await;
    }

    #[tokio::test]
    async fn reconnect_makes_the_endpoint_afresh_closing_the_old_one_first() {
        let backend = echo_backend().await;
        let (make, made) = maker(Duration::ZERO);
        let tunnel = Tunnel::open_with(make, Duration::ZERO, backend.addr()).await.unwrap();
        let (port, path) = (tunnel.port(), tunnel.path().to_owned());
        let mut before = ask(&tunnel).await;
        echoes(&mut before, b"before").await;

        tunnel.reconnect().await;
        assert_eq!(tunnel.status().phase, Phase::Idle, "nothing failed");
        assert!(tunnel.status().error.is_empty());
        {
            let made = made.lock().unwrap();
            assert_eq!(made.len(), 2, "a new endpoint");
            assert!(made[0].is_closed(), "the old one closed");
            assert!(!made[1].is_closed());
            assert_eq!(made[0].id(), made[1].id(), "the same device");
        }
        // The same port and path, a new connection on the new endpoint.
        assert_eq!((tunnel.port(), tunnel.path()), (port, path.as_str()));
        let mut after = ask(&tunnel).await;
        echoes(&mut after, b"after").await;
        assert_eq!(tunnel.status().phase, Phase::Connected);
        tunnel.close().await;
        assert!(made.lock().unwrap().iter().all(Endpoint::is_closed));
        // Nothing is made after it's closed.
        tunnel.reconnect().await;
        assert_eq!(made.lock().unwrap().len(), 2, "closed stays closed");
    }

    #[tokio::test]
    async fn a_young_endpoint_is_only_told_the_network_changed() {
        let backend = echo_backend().await;
        let (make, made) = maker(Duration::ZERO);
        let tunnel = Tunnel::open_with(make, Duration::from_secs(3600), backend.addr()).await.unwrap();
        let mut socket = ask(&tunnel).await;
        echoes(&mut socket, b"one").await;
        for _ in 0..3 {
            tunnel.reconnect().await;
        }
        assert_eq!(made.lock().unwrap().len(), 1, "never made again in a row");
        let mut socket = ask(&tunnel).await;
        echoes(&mut socket, b"two").await;
        tunnel.close().await;
    }

    #[test]
    fn rebuilds_back_off_to_five_minutes() {
        let base = Duration::from_secs(5);
        let ages: Vec<u64> = (0..9).map(|n| rebuild_age(base, n).as_secs()).collect();
        assert_eq!(ages, [5, 10, 20, 40, 80, 160, 300, 300, 300]);
        assert_eq!(rebuild_age(base, u32::MAX), REBUILD_AFTER_MAX);
        assert_eq!(rebuild_age(Duration::ZERO, 5), Duration::ZERO);
        assert_eq!(rebuild_age(Duration::from_secs(3600), 3), Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn rebuilds_no_connection_followed_wait_longer_until_one_does() {
        let backend = echo_backend().await;
        let (make, made) = maker(Duration::ZERO);
        // Wide margins: a busy machine stretches every sleep.
        let base = Duration::from_secs(1);
        let tunnel = Tunnel::open_with(make, base, backend.addr()).await.unwrap();
        let count = || made.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        tunnel.reconnect().await;
        assert_eq!(count(), 2, "old enough: made afresh");
        // Nothing connected on it: the next waits twice as long.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        tunnel.reconnect().await;
        assert_eq!(count(), 2, "backing off");
        tokio::time::sleep(Duration::from_millis(1000)).await;
        tunnel.reconnect().await;
        assert_eq!(count(), 3, "old enough again");
        // A connection made: back to the first wait.
        let mut socket = ask(&tunnel).await;
        echoes(&mut socket, b"hello").await;
        tokio::time::sleep(Duration::from_millis(1100)).await;
        tunnel.reconnect().await;
        assert_eq!(count(), 4, "reset by the connection");
        tunnel.close().await;
    }

    #[tokio::test]
    async fn an_endpoint_that_just_connected_isnt_made_afresh() {
        let backend = echo_backend().await;
        let (make, made) = maker(Duration::ZERO);
        let tunnel = Tunnel::open_with(make, Duration::from_millis(1500), backend.addr()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(1600)).await;
        // Old, but it connects now (back on the network after a while).
        let mut socket = ask(&tunnel).await;
        echoes(&mut socket, b"works").await;
        tunnel.reconnect().await;
        assert_eq!(made.lock().unwrap().len(), 1, "it works: only told the network changed");
        let mut socket = ask(&tunnel).await;
        echoes(&mut socket, b"still").await;
        tunnel.close().await;
    }

    #[tokio::test]
    async fn a_reconnect_during_another_joins_it_and_makes_no_second_endpoint() {
        let backend = echo_backend().await;
        let (make, made) = maker(Duration::from_millis(400));
        let tunnel = Arc::new(Tunnel::open_with(make, Duration::ZERO, backend.addr()).await.unwrap());
        let first = tokio::spawn({
            let tunnel = tunnel.clone();
            async move { tunnel.reconnect().await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        // Retries while the first rebuilds: they wait for it, nothing more.
        tokio::join!(tunnel.reconnect(), tunnel.reconnect());
        assert_eq!(made.lock().unwrap().len(), 2, "returned only once the rebuild under way had made its endpoint");
        first.await.unwrap();
        assert_eq!(made.lock().unwrap().len(), 2, "one rebuild, not three");
        let mut socket = ask(&tunnel).await;
        echoes(&mut socket, b"once").await;
        assert_eq!(made.lock().unwrap().len(), 2);
        // Once it has ended, a reconnect is its own again.
        tunnel.reconnect().await;
        assert_eq!(made.lock().unwrap().len(), 3);
        tunnel.close().await;
    }

    #[tokio::test]
    async fn close_returns_bounded_even_mid_dial_and_the_endpoint_still_closes() {
        // A machine that never answers: a dial on it holds a connection
        // whose draining keeps the endpoint's close waiting.
        let dead = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let nobody = SecretKey::generate().public();
        let addr = EndpointAddr::new(nobody).with_ip_addr(dead.local_addr().unwrap());
        let (make, made) = maker(Duration::ZERO);
        let tunnel = Tunnel::open_with(make, REBUILD_AFTER, addr).await.unwrap();
        let mut socket = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
        socket.write_all(format!("GET {} HTTP/1.1\r\n", tunnel.path()).as_bytes()).await.unwrap();
        for _ in 0..100 {
            if tunnel.status().phase == Phase::Dialing {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(tunnel.status().phase, Phase::Dialing);
        let begun = Instant::now();
        tunnel.close().await;
        assert!(begun.elapsed() <= CLOSE_TIMEOUT + Duration::from_millis(500), "took {:?}", begun.elapsed());
        // Not left half-closed: it finishes closing in the background.
        let endpoint = made.lock().unwrap()[0].clone();
        let closed = tokio::time::timeout(Duration::from_secs(20), async {
            while !endpoint.is_closed() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        assert!(closed.is_ok(), "still open {:?} after close", begun.elapsed());
    }

    #[tokio::test]
    async fn a_socket_during_a_reconnect_waits_for_the_new_endpoint() {
        let backend = echo_backend().await;
        let (make, made) = maker(Duration::from_millis(400));
        let tunnel = Arc::new(Tunnel::open_with(make, Duration::ZERO, backend.addr()).await.unwrap());
        let reconnecting = tokio::spawn({
            let tunnel = tunnel.clone();
            async move { tunnel.reconnect().await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        let status = tunnel.status();
        assert_eq!((status.phase, status.failure), (Phase::Idle, Failure::None), "reconnecting, not failed");
        let mut socket = ask(&tunnel).await;
        echoes(&mut socket, b"waited").await;
        reconnecting.await.unwrap();
        assert_eq!(made.lock().unwrap().len(), 2);
        tunnel.close().await;
    }

    #[test]
    fn keys_and_addresses_parse() {
        let key = generate_key();
        assert_eq!(key.len(), 64);
        let id = parse_key(&key).unwrap().public().to_string();
        let addr =
            backend_addr(&id, &["https://relay.example.com".to_owned()], &["127.0.0.1:4919".to_owned(), "[::1]:4919".to_owned()]).unwrap();
        assert_eq!(addr.relay_urls().count(), 1);
        let ips: Vec<String> = addr.ip_addrs().map(|ip| ip.to_string()).collect();
        assert_eq!(ips.len(), 2);
        assert!(ips.contains(&"127.0.0.1:4919".to_owned()));
        assert!(backend_addr("nope", &[], &[]).is_err());
        assert!(backend_addr(&id, &[], &["nowhere".to_owned()]).is_err());
        assert!(parse_key("abcd").is_err());
    }

    #[test]
    fn pairing_ids_are_both_endpoint_ids_in_hex() {
        let device = generate_key();
        let backend = parse_key(&generate_key()).unwrap().public();
        let (device_id, backend_id) = pairing_ids(&device, &backend.to_string()).unwrap();
        assert_eq!(device_id, hex::encode(parse_key(&device).unwrap().public().as_bytes()));
        assert_eq!(backend_id, hex::encode(backend.as_bytes()));
        assert_eq!(backend_id.len(), 64);
        // The same id written in hex reads the same.
        assert_eq!(pairing_ids(&device, &backend_id.to_uppercase()).unwrap().1, backend_id);
        assert!(pairing_ids("abcd", &backend_id).is_err());
        assert!(pairing_ids(&device, "nope").is_err());
    }

    /// A relay on this computer, and a backend homed on it that echoes.
    async fn relayed_backend() -> (Endpoint, RelayUrl, impl std::any::Any) {
        let (_map, url, server) = iroh::test_utils::run_relay_server().await.unwrap();
        let backend = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Custom(iroh::RelayMap::from(url.clone())))
            .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
            .alpns(vec![ALPN_COMPRESSED.to_vec(), ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        serve_echo(&backend);
        tokio::time::timeout(Duration::from_secs(10), backend.online()).await.expect("the backend reaches the relay");
        (backend, url, server)
    }

    /// The backend's direct address on this computer.
    fn direct_addr(backend: &Endpoint) -> SocketAddr {
        let port = backend.bound_sockets().iter().find(|addr| addr.is_ipv4()).unwrap().port();
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }

    /// Device endpoints homed on [relay], as [maker] makes them.
    fn relayed_maker(relay: RelayUrl) -> (MakeEndpoint, Arc<Mutex<Vec<Endpoint>>>) {
        let key = parse_key(&generate_key()).unwrap();
        let made = Arc::new(Mutex::new(Vec::<Endpoint>::new()));
        let make: MakeEndpoint = Arc::new({
            let made = made.clone();
            move || {
                let (key, made, relay) = (key.clone(), made.clone(), relay.clone());
                // The stalled endpoint may still be closing, in the
                // background ([close_endpoint]): its connection drains.
                Box::pin(async move {
                    let endpoint = Endpoint::builder(presets::Minimal)
                        .relay_mode(RelayMode::Custom(iroh::RelayMap::from(relay)))
                        .ca_tls_config(iroh::tls::CaTlsConfig::insecure_skip_verify())
                        .secret_key(key)
                        .bind()
                        .await?;
                    made.lock().unwrap().push(endpoint.clone());
                    Ok(endpoint)
                })
            }
        });
        (make, made)
    }

    /// A direct path that answers from the wrong address (#392), as a Mac
    /// backend reached at an address that isn't its route back does: what
    /// the device sends to the returned address reaches [backend]; the
    /// backend's answers come back from another port, and what the device
    /// sends there reaches the backend from another socket, which a
    /// backend mid-handshake discards.
    async fn wrong_source_path(backend: SocketAddr) -> (SocketAddr, JoinHandle<()>) {
        use tokio::net::UdpSocket;
        let bind = || UdpSocket::bind((Ipv4Addr::LOCALHOST, 0));
        let (entry, side, up, up_side) = (bind().await.unwrap(), bind().await.unwrap(), bind().await.unwrap(), bind().await.unwrap());
        let address = entry.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut device = None;
            let (mut a, mut b, mut c, mut d) = (vec![0u8; 65536], vec![0u8; 65536], vec![0u8; 65536], vec![0u8; 65536]);
            loop {
                tokio::select! {
                    Ok((read, from)) = entry.recv_from(&mut a) => {
                        device = Some(from);
                        let _ = up.send_to(&a[..read], backend).await;
                    }
                    Ok((read, from)) = side.recv_from(&mut b) => {
                        device = Some(from);
                        let _ = up_side.send_to(&b[..read], backend).await;
                    }
                    Ok((read, _)) = up.recv_from(&mut c) => if let Some(device) = device {
                        let _ = side.send_to(&c[..read], device).await;
                    },
                    Ok((read, _)) = up_side.recv_from(&mut d) => if let Some(device) = device {
                        let _ = side.send_to(&d[..read], device).await;
                    },
                }
            }
        });
        (address, task)
    }

    #[tokio::test]
    async fn a_direct_path_that_never_confirms_falls_back_to_the_relay_within_seconds() {
        let (backend, relay, _server) = relayed_backend().await;
        let (stalls, proxy) = wrong_source_path(direct_addr(&backend)).await;
        let code = EndpointAddr::new(backend.id()).with_relay_url(relay.clone()).with_ip_addr(stalls);
        let (make, made) = relayed_maker(relay);
        let tunnel = Tunnel::open_with(make, REBUILD_AFTER, code).await.unwrap();
        let begun = Instant::now();
        let _socket = ask(&tunnel).await;
        let took = begun.elapsed();
        assert!(took >= CONFIRM_WITHIN && took < CONFIRM_WITHIN + Duration::from_secs(3), "took {took:?}");
        assert!(tunnel.shared.relay_first.load(Ordering::Relaxed));
        // Made afresh for the relay, the stalled one closed first.
        assert_eq!(made.lock().unwrap().len(), 2);
        round_trip(&tunnel).await;

        // Later dials go through the relay at once.
        tunnel.reconnect().await;
        let begun = Instant::now();
        let _socket = ask(&tunnel).await;
        assert!(begun.elapsed() < CONFIRM_WITHIN, "took {:?}", begun.elapsed());
        tunnel.close().await;
        proxy.abort();
    }

    #[tokio::test]
    async fn a_direct_path_that_confirms_is_kept() {
        let (backend, relay, _server) = relayed_backend().await;
        let code = EndpointAddr::new(backend.id()).with_relay_url(relay.clone()).with_ip_addr(direct_addr(&backend));
        let (make, made) = relayed_maker(relay);
        let tunnel = Tunnel::open_with(make, REBUILD_AFTER, code).await.unwrap();
        let begun = Instant::now();
        let _socket = ask(&tunnel).await;
        assert!(begun.elapsed() < CONFIRM_WITHIN, "took {:?}", begun.elapsed());
        assert_eq!(tunnel.status().path, Path::Direct);
        assert!(!tunnel.shared.relay_first.load(Ordering::Relaxed));
        assert_eq!(made.lock().unwrap().len(), 1);
        tunnel.close().await;
    }

    /// #413: what arrives counts as it arrives, before a whole message is
    /// in or decompressed, so the app can tell a slow link that's
    /// delivering from a dead one.
    #[tokio::test]
    async fn the_status_counts_bytes_as_they_arrive() {
        for alpn in [ALPN, ALPN_COMPRESSED] {
            let backend = echo_backend_with(&[alpn]).await;
            let tunnel = Tunnel::open(local_device().await, backend.addr()).await.unwrap();
            assert_eq!(tunnel.status().received, 0);
            let mut socket = ask(&tunnel).await;
            let after_ask = tunnel.status().received;
            assert!(after_ask > 0, "{alpn:?}");
            // Bytes that don't compress, so both ways count them all.
            let mut state = 0x2545_f491_u32;
            let bytes: Vec<u8> = (0..100_000)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 17;
                    state ^= state << 5;
                    state as u8
                })
                .collect();
            echoes(&mut socket, &bytes).await;
            let received = tunnel.status().received.wrapping_sub(after_ask);
            assert!(received as usize >= bytes.len() * 9 / 10, "{alpn:?}: {received}");
            // The link's count, QUIC's own bytes and all, follows within
            // half a second.
            tokio::time::sleep(Duration::from_millis(700)).await;
            let link = tunnel.status().link_received;
            assert!(link >= tunnel.status().received, "{alpn:?}: link {link}");
            tunnel.close().await;
        }
    }

    /// A UDP path to [backend] that carries everything while [on], and
    /// nothing otherwise: a link that's there, then isn't.
    async fn switchable_path(backend: SocketAddr) -> (SocketAddr, Arc<AtomicBool>, JoinHandle<()>) {
        use tokio::net::UdpSocket;
        let entry = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let up = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = entry.local_addr().unwrap();
        let on = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn({
            let on = on.clone();
            async move {
                let mut device = None;
                let (mut a, mut b) = (vec![0u8; 65536], vec![0u8; 65536]);
                loop {
                    tokio::select! {
                        Ok((read, from)) = entry.recv_from(&mut a) => {
                            device = Some(from);
                            if on.load(Ordering::Relaxed) {
                                let _ = up.send_to(&a[..read], backend).await;
                            }
                        }
                        Ok((read, _)) = up.recv_from(&mut b) => if let Some(device) = device && on.load(Ordering::Relaxed) {
                            let _ = entry.send_to(&b[..read], device).await;
                        },
                    }
                }
            }
        });
        (address, on, task)
    }

    /// How long a socket waits before the tunnel gives up dialing for it
    /// and closes it.
    async fn given_up_after(tunnel: &Tunnel) -> Duration {
        let mut socket = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
        let begun = Instant::now();
        socket.write_all(format!("GET {} HTTP/1.1\r\n", tunnel.path()).as_bytes()).await.unwrap();
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), socket.read_to_end(&mut rest)).await.expect("given up").ok();
        begun.elapsed()
    }

    /// #413: on a weak link a dial that times out gives the next one longer
    /// (DIAL_TIMEOUTS), up to the last; one made, and the first again.
    #[tokio::test]
    async fn dials_that_time_out_get_longer_each_time_and_the_first_again_once_one_is_made() {
        let backend = echo_backend().await;
        let (path, on, proxy) = switchable_path(direct_addr(&backend)).await;
        let (make, _made) = maker(Duration::ZERO);
        let tunnel = Tunnel::open_with(make, REBUILD_AFTER, EndpointAddr::new(backend.id()).with_ip_addr(path)).await.unwrap();
        let ms = |duration: Duration| u32::try_from(duration.as_millis()).unwrap();
        assert_eq!(tunnel.status().dial_timeout_ms, ms(DIAL_TIMEOUTS[0]));
        for expected in [DIAL_TIMEOUTS[0], DIAL_TIMEOUTS[1], DIAL_TIMEOUTS[2], DIAL_TIMEOUTS[2]] {
            let took = given_up_after(&tunnel).await;
            assert!(took >= expected && took < expected + Duration::from_millis(400), "{took:?} for {expected:?}");
            assert_eq!(tunnel.status().phase, Phase::Failed);
        }
        assert_eq!(tunnel.status().dial_timeout_ms, ms(DIAL_TIMEOUTS[2]));
        // The link comes back: made, and patient no more.
        on.store(true, Ordering::Relaxed);
        let mut socket = ask(&tunnel).await;
        echoes(&mut socket, b"back").await;
        assert_eq!(tunnel.status().dial_timeout_ms, ms(DIAL_TIMEOUTS[0]));
        tunnel.close().await;
        proxy.abort();
    }
}
