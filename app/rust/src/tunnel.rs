//! The private tunnel's app side (docs: private-tunnel.md): the app reaches
//! its backend only through iroh. A [`Tunnel`] listens on a random loopback
//! port; each TCP connection the app opens there (one WebSocket) becomes a
//! new bidirectional stream on one iroh connection to the backend, carrying
//! exactly the socket's bytes both ways. The protocol code above it is the
//! same as over a plain socket.

use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayUrl, SecretKey, Watcher,
    endpoint::{Connection, ConnectionError, presets},
};
use tokio::{
    io::{AsyncWriteExt, copy},
    net::{TcpListener, TcpStream},
    sync::Mutex as AsyncMutex,
    task::JoinHandle,
};

/// The backend's protocol on iroh, both sides (private-tunnel.md).
pub const ALPN: &[u8] = b"claktory/1";

/// How long a dial may take before the app says why it can't connect,
/// rather than waiting on.
pub const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

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
    shared: Arc<Shared>,
    accept: JoinHandle<()>,
    watch: JoinHandle<()>,
}

struct Shared {
    endpoint: Endpoint,
    backend: EndpointAddr,
    /// The connection streams open on; dialled by the first socket that
    /// finds none open.
    connection: AsyncMutex<Option<Connection>>,
    status: Mutex<Status>,
}

impl Tunnel {
    /// Listens on 127.0.0.1 at a random port and forwards to [backend]
    /// through [endpoint]. Nothing is dialled until the app connects.
    pub async fn open(endpoint: Endpoint, backend: EndpointAddr) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared { endpoint, backend, connection: AsyncMutex::new(None), status: Mutex::new(Status::default()) });
        let accept = tokio::spawn({
            let shared = shared.clone();
            async move {
                while let Ok((socket, _)) = listener.accept().await {
                    let _ = socket.set_nodelay(true);
                    tokio::spawn(forward(socket, shared.clone()));
                }
            }
        });
        // Whether this device reaches its home relay, as it changes.
        let watch = tokio::spawn({
            let shared = shared.clone();
            async move {
                let mut relays = shared.endpoint.home_relay_status();
                loop {
                    let connected = relays.get().iter().any(|relay| relay.is_connected());
                    shared.status.lock().unwrap().relay_connected = connected;
                    if relays.updated().await.is_err() {
                        break;
                    }
                }
            }
        });
        Ok(Self { port, shared, accept, watch })
    }

    /// The loopback port the app connects to: ws://127.0.0.1:<port>.
    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn status(&self) -> Status {
        self.shared.status.lock().unwrap().clone()
    }

    /// After the app was suspended, or the network changed under it: the
    /// connection may be dead without knowing it yet, so it's closed (the
    /// next socket dials afresh, unless a dial is under way) and iroh looks
    /// at the network again. A failure from before says nothing about now.
    pub async fn reconnect(&self) {
        if let Ok(mut slot) = self.shared.connection.try_lock()
            && let Some(connection) = slot.take()
        {
            connection.close(0u32.into(), b"");
            let mut status = self.shared.status.lock().unwrap();
            status.phase = Phase::Idle;
            status.path = Path::None;
            status.error.clear();
            status.failure = Failure::None;
        }
        self.shared.endpoint.network_change().await;
    }

    /// Stops listening and closes the connection and the endpoint.
    pub async fn close(&self) {
        self.accept.abort();
        self.watch.abort();
        if let Some(connection) = self.shared.connection.lock().await.take() {
            connection.close(0u32.into(), b"closed");
        }
        self.shared.endpoint.close().await;
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.accept.abort();
        self.watch.abort();
    }
}

/// One app socket: a new stream on the connection (dialling it if needed),
/// bytes copied both ways until either side ends. A socket the tunnel
/// can't carry is closed at once, and the status says why.
async fn forward(socket: TcpStream, shared: Arc<Shared>) {
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
    let up = async {
        let _ = copy(&mut read, &mut send).await;
        let _ = send.finish();
    };
    let down = async {
        let _ = copy(&mut receive, &mut write).await;
        let _ = write.shutdown().await;
    };
    tokio::join!(up, down);
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
    shared.status.lock().unwrap().phase = Phase::Dialing;
    let dial = shared.endpoint.connect(shared.backend.clone(), ALPN);
    let connection = match tokio::time::timeout(DIAL_TIMEOUT, dial).await {
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
                shared.failed("Nothing answered within 20 seconds: the machine may be off, asleep or offline.", Failure::Unreachable);
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
        let endpoint = Endpoint::builder(presets::Minimal).relay_mode(RelayMode::Disabled).alpns(vec![ALPN.to_vec()]).bind().await.unwrap();
        let server = endpoint.clone();
        tokio::spawn(async move {
            while let Some(incoming) = server.accept().await {
                tokio::spawn(async move {
                    let connection = incoming.await.unwrap();
                    while let Ok((mut send, mut receive)) = connection.accept_bi().await {
                        tokio::spawn(async move {
                            let _ = copy(&mut receive, &mut send).await;
                            let _ = send.finish();
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

    #[tokio::test]
    async fn sockets_become_streams_on_one_connection() {
        let backend = echo_backend().await;
        let tunnel = Tunnel::open(local_device().await, backend.addr()).await.unwrap();
        assert_eq!(tunnel.status().phase, Phase::Idle, "nothing dials before the app connects");

        // Two sockets at once, each its own stream, bytes both ways.
        let mut first = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
        let mut second = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
        first.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
        second.write_all(b"second").await.unwrap();
        let mut got = [0u8; 16];
        first.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"GET / HTTP/1.1\r\n");
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
    async fn reconnect_dials_afresh_without_a_failure() {
        let backend = echo_backend().await;
        let tunnel = Tunnel::open(local_device().await, backend.addr()).await.unwrap();
        let mut socket = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
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
        let mut again = TcpStream::connect(("127.0.0.1", tunnel.port())).await.unwrap();
        again.write_all(b"two").await.unwrap();
        again.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"two");
        assert_eq!(tunnel.status().phase, Phase::Connected);
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
        socket.write_all(b"hello").await.unwrap();
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

    #[test]
    fn keys_and_addresses_parse() {
        let key = generate_key();
        assert_eq!(key.len(), 64);
        let id = parse_key(&key).unwrap().public().to_string();
        let addr = backend_addr(&id, &["https://relay.example.com".to_owned()], &["127.0.0.1:4919".to_owned(), "[::1]:4919".to_owned()]).unwrap();
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
