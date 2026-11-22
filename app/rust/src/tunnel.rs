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
        atomic::{AtomicU32, Ordering},
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
/// rather than waiting on. Under the app's own 10 s connect window, so a
/// slow dial fails here first and the next try isn't queued behind it.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(8);

/// How long a socket has to say what it asks for, and how much it may say
/// before its request line ends.
#[cfg(not(test))]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
const REQUEST_TIMEOUT: Duration = Duration::from_millis(300);
const REQUEST_LINE_MAX: usize = 1024;

/// How long the old endpoint has to close before a new one is made in its
/// place: its sockets may be dead (iOS reclaims a suspended app's), so it's
/// not waited on longer (the Dart side bounds closing a tunnel the same).
/// One that hasn't closed by then (still dialling its home relay, say) is
/// said so in the log and dropped, which aborts it and its relay
/// connections (iroh logs "Endpoint dropped without calling
/// `Endpoint::close`", expected here); the new one may then meet it at the
/// relay for a moment, where the newer wins.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

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

/// A device endpoint with [key], on n0's relays (Matt's choice for now) and
/// with iroh's address lookup, so a backend whose home relay changed is
/// still found.
pub async fn device_endpoint(key: SecretKey) -> anyhow::Result<Endpoint> {
    Ok(Endpoint::builder(presets::N0).secret_key(key).bind().await?)
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
    /// When a connection was last made: the endpoint works, whatever its
    /// age, and isn't made afresh for [rebuild_age] after.
    connected_at: Mutex<Option<Instant>>,
    /// Follows the current endpoint's home relay.
    relay_watch: Mutex<Option<JoinHandle<()>>>,
    backend: EndpointAddr,
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
        let endpoint = make().await?;
        Self::start(endpoint, Some(make), rebuild_after, backend).await
    }

    async fn start(
        endpoint: Endpoint,
        make: Option<MakeEndpoint>,
        rebuild_after: Duration,
        backend: EndpointAddr,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
        let port = listener.local_addr()?.port();
        // A key's 32 bytes are as random as any: the system's CSPRNG.
        let path: Arc<str> = format!("/{}", hex::encode(SecretKey::generate().to_bytes())).into();
        let shared = Arc::new(Shared {
            endpoint: AsyncMutex::new(Slot { endpoint: Some(endpoint.clone()), made: Instant::now(), closed: false }),
            make,
            rebuild_after,
            unanswered: AtomicU32::new(0),
            connected_at: Mutex::new(None),
            relay_watch: Mutex::new(None),
            backend,
            connection: AsyncMutex::new(None),
            status: Mutex::new(Status::default()),
            forwards: AsyncMutex::new(None),
            cancel_dials: tokio::sync::Notify::new(),
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
        self.shared.status.lock().unwrap().clone()
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
    /// followed ([rebuild_age]).
    pub async fn reconnect(&self) {
        let shared = &self.shared;
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
        // Since it was made or last worked, whichever is later.
        let quiet = match *shared.connected_at.lock().unwrap() {
            Some(at) => slot.made.elapsed().min(at.elapsed()),
            None => slot.made.elapsed(),
        };
        let young = quiet < rebuild_age(shared.rebuild_after, shared.unanswered.load(Ordering::Relaxed));
        let Some(make) = shared.make.clone().filter(|_| !young || slot.endpoint.is_none()) else {
            if let Some(endpoint) = slot.endpoint.clone() {
                drop(slot);
                endpoint.network_change().await;
            }
            return;
        };
        if let Some(old) = slot.endpoint.take() {
            shared.stop_relay_watch();
            if tokio::time::timeout(CLOSE_TIMEOUT, old.close()).await.is_err() {
                // Logcat's only (android.rs): nothing reads it elsewhere.
                #[cfg(any(target_os = "android", test))]
                tracing::warn!("The old endpoint didn't close within {} s; making the new one anyway.", CLOSE_TIMEOUT.as_secs());
            }
        }
        shared.status.lock().unwrap().relay_connected = false;
        match make().await {
            Ok(endpoint) => {
                shared.watch_relay(&endpoint);
                slot.endpoint = Some(endpoint);
                slot.made = Instant::now();
                shared.unanswered.fetch_add(1, Ordering::Relaxed);
            }
            // Left for the next dial to make; until then, nothing failed.
            Err(_) => slot.endpoint = None,
        }
    }

    /// Stops listening and closes the connection and the endpoint.
    pub async fn close(&self) {
        self.accept.abort();
        self.shared.stop_relay_watch();
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
            endpoint.close().await;
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
        let dial = endpoint.connect(self.0.backend.clone(), ALPN_FORWARD);
        let connection = match tokio::time::timeout(DIAL_TIMEOUT, dial).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) if refused_alpn(&error) => return Err(ForwardingFailure::Unsupported),
            Ok(Err(error)) => return Err(ForwardingFailure::Failed(explain_connect(&error).0)),
            Err(_) => return Err(ForwardingFailure::Failed(format!("Nothing answered within {} seconds.", DIAL_TIMEOUT.as_secs()))),
        };
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
    let (mut send, mut receive) = match connection.open_bi().await {
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
            let _ = copy(&mut ZstdDecoder::new(BufReader::new(receive)), &mut write).await;
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
        let _ = copy(&mut receive, &mut write).await;
        let _ = write.shutdown().await;
    };
    tokio::join!(up, down);
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
    Ok(endpoint.connect_with_opts(shared.backend.clone(), ALPN_COMPRESSED, options).await?.await?)
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
    let dialled = tokio::select! {
        dialled = tokio::time::timeout(DIAL_TIMEOUT, dial(shared, &endpoint)) => dialled,
        // `reconnect`: this dial may be stuck on a path that's gone. Give
        // up quietly; the next socket dials afresh on what the network is
        // now.
        _ = shared.cancel_dials.notified() => {
            let mut status = shared.status.lock().unwrap();
            status.phase = Phase::Idle;
            status.path = Path::None;
            *slot = None;
            return None;
        }
    };
    let connection = match dialled {
        Ok(Ok(connection)) => connection,
        Ok(Err(error)) => {
            let (said, failure) = explain_connect(&error);
            shared.failed(&said, failure);
            *slot = None;
            return None;
        }
        Err(_) => {
            let relay = shared.status.lock().unwrap().relay_connected;
            if relay {
                let said = format!("Nothing answered within {} seconds: the machine may be off, asleep or offline.", DIAL_TIMEOUT.as_secs());
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
                let path = path_of(&connection);
                shared.status.lock().unwrap().path = path;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    };
    let closed = connection.closed();
    tokio::select! {
        error = closed => {
            let (said, failure) = explain_closed(&error);
            let mut status = shared.status.lock().unwrap();
            status.path = Path::None;
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
        let endpoint = make().await?;
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
        let watch = tokio::spawn(async move {
            loop {
                let connected = relays.get().iter().any(|relay| relay.is_connected());
                let Some(shared) = shared.upgrade() else { break };
                shared.status.lock().unwrap().relay_connected = connected;
                drop(shared);
                if relays.updated().await.is_err() {
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
        status.error.clear();
        status.failure = Failure::None;
    }

    fn failed(&self, said: &str, failure: Failure) {
        let mut status = self.status.lock().unwrap();
        status.phase = Phase::Failed;
        status.path = Path::None;
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

    /// A backend stand-in on this machine, no relays: echoes every stream.
    async fn echo_backend() -> Endpoint {
        echo_backend_with(&[ALPN]).await
    }

    /// An echoing backend with these ALPNs: on `claktory/2` it decompresses
    /// what comes and compresses what it sends back, as factoryd does.
    async fn echo_backend_with(alpns: &[&[u8]]) -> Endpoint {
        let alpns = alpns.iter().map(|alpn| alpn.to_vec()).collect();
        let endpoint = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).alpns(alpns).bind().await.unwrap();
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
        endpoint
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
        let base = Duration::from_millis(300);
        let tunnel = Tunnel::open_with(make, base, backend.addr()).await.unwrap();
        let count = || made.lock().unwrap().len();
        tokio::time::sleep(Duration::from_millis(350)).await;
        tunnel.reconnect().await;
        assert_eq!(count(), 2, "old enough: made afresh");
        // Nothing connected on it: the next waits twice as long.
        tokio::time::sleep(Duration::from_millis(350)).await;
        tunnel.reconnect().await;
        assert_eq!(count(), 2, "backing off");
        tokio::time::sleep(Duration::from_millis(300)).await;
        tunnel.reconnect().await;
        assert_eq!(count(), 3, "old enough again");
        // A connection made: back to the first wait.
        let mut socket = ask(&tunnel).await;
        echoes(&mut socket, b"hello").await;
        tokio::time::sleep(Duration::from_millis(350)).await;
        tunnel.reconnect().await;
        assert_eq!(count(), 4, "reset by the connection");
        tunnel.close().await;
    }

    #[tokio::test]
    async fn an_endpoint_that_just_connected_isnt_made_afresh() {
        let backend = echo_backend().await;
        let (make, made) = maker(Duration::ZERO);
        let tunnel = Tunnel::open_with(make, Duration::from_millis(500), backend.addr()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(600)).await;
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
}
