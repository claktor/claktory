//! The private tunnel, for Dart: a device key, and a tunnel to a backend
//! that the app reaches at ws://127.0.0.1:<port><path>.

use std::sync::{Arc, Mutex};

use flutter_rust_bridge::frb;

use crate::forward;
use crate::tunnel::{self, Failure, Path, Phase, Reach, RelayTrouble};

/// The tunnels' own runtime: iroh's tasks outlive any one call from Dart.
/// Made on first use, and made afresh by [fresh_runtime] when it stops
/// answering ([runtime_answers]), as relaunching the app would (#214).
static RUNTIME: Mutex<Option<tokio::runtime::Runtime>> = Mutex::new(None);

/// The current runtime's handle, making it if there's none.
fn runtime() -> tokio::runtime::Handle {
    let mut slot = RUNTIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    slot.get_or_insert_with(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("tunnel")
            .enable_all()
            .build()
            .expect("the tunnel's runtime")
    })
    .handle()
    .clone()
}

/// Whether the tunnels' runtime runs a task: one that never answers (the
/// app times this out) is stuck, and only [fresh_runtime] gets past it.
pub async fn runtime_answers() -> bool {
    runtime().spawn(async {}).await.is_ok()
}

/// Ends the tunnels' runtime without waiting for it, and with it every
/// tunnel it ran (their tasks dropped, their endpoints with them); the next
/// tunnel opens on a new one. What relaunching the app does, without
/// relaunching it: only for a runtime that stopped answering (#214).
#[frb(sync)]
pub fn fresh_runtime() {
    let old = RUNTIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
    if let Some(old) = old {
        old.shutdown_background();
    }
}

/// A tunnel to one backend, on the runtime it was opened on: once that's
/// ended ([fresh_runtime]), its calls return at once.
#[frb(opaque)]
pub struct TunnelHandle {
    tunnel: Arc<tunnel::Tunnel>,
    runtime: tokio::runtime::Handle,
}

/// Opens a tunnel to the backend [endpoint_id] (its relays [relays] and
/// direct addresses [addresses], from its pairing code), as the device
/// [device_key]. It listens at once and dials when the app first connects.
pub async fn open_tunnel(
    device_key: String,
    endpoint_id: String,
    relays: Vec<String>,
    addresses: Vec<String>,
) -> anyhow::Result<TunnelHandle> {
    // Without it iroh would resolve through public fallbacks, not the
    // phone's own DNS; and a Dart that loaded a second copy of this
    // library would find it missing here.
    #[cfg(target_os = "android")]
    crate::android::ready()?;
    let runtime = runtime();
    runtime
        .clone()
        .spawn(async move {
            let key = tunnel::parse_key(&device_key)?;
            let backend = tunnel::backend_addr(&endpoint_id, &relays, &addresses)?;
            // Made again by reconnect, after the app was away or the network
            // changed (tunnel.rs).
            let make: tunnel::MakeEndpoint = Arc::new(move || Box::pin(tunnel::device_endpoint(key.clone())));
            let tunnel = tunnel::Tunnel::open_with(make, tunnel::REBUILD_AFTER, backend).await?;
            Ok(TunnelHandle { tunnel: Arc::new(tunnel), runtime })
        })
        .await?
}

/// The endpoint ids, as lowercase hex, that the number a device shows while
/// it waits to be allowed is worked out from: [device]'s, from its key
/// [device_key], and the [backend]'s, from its pairing code's `e=`.
pub struct PairingIds {
    pub device: String,
    pub backend: String,
}

#[frb(sync)]
pub fn pairing_ids(device_key: String, endpoint_id: String) -> anyhow::Result<PairingIds> {
    let (device, backend) = tunnel::pairing_ids(&device_key, &endpoint_id)?;
    Ok(PairingIds { device, backend })
}

impl TunnelHandle {
    /// Where the app connects: ws://127.0.0.1:<port><path>.
    #[frb(sync)]
    pub fn port(&self) -> u16 {
        self.tunnel.port()
    }

    /// The path the app asks for at [port]: `/<secret>`, the tunnel's own.
    /// Nothing else there is forwarded, so no other program on this device
    /// passes for it. Never shown, saved or logged.
    #[frb(sync)]
    pub fn path(&self) -> String {
        self.tunnel.path().to_owned()
    }

    #[frb(sync)]
    pub fn status(&self) -> TunnelStatus {
        let status = self.tunnel.status();
        TunnelStatus {
            phase: match status.phase {
                Phase::Idle => TunnelPhase::Idle,
                Phase::Dialing => TunnelPhase::Dialing,
                Phase::Connected => TunnelPhase::Connected,
                Phase::Failed => TunnelPhase::Failed,
            },
            path: match status.path {
                Path::None => TunnelPath::None,
                Path::Direct => TunnelPath::Direct,
                Path::Relayed => TunnelPath::Relayed,
            },
            error: status.error,
            failure: match status.failure {
                Failure::None => TunnelFailure::None,
                Failure::Unreachable => TunnelFailure::Unreachable,
                Failure::Refused => TunnelFailure::Refused,
                Failure::Offline => TunnelFailure::Offline,
                Failure::Other => TunnelFailure::Other,
            },
            relay_connected: status.relay_connected,
            relay_up_ms: status.relay_up_ms,
            relay_trouble: match status.relay_trouble {
                RelayTrouble::None => TunnelRelayTrouble::None,
                RelayTrouble::Unchosen => TunnelRelayTrouble::Unchosen,
                RelayTrouble::Dns => TunnelRelayTrouble::Dns,
                RelayTrouble::Timeout => TunnelRelayTrouble::Timeout,
                RelayTrouble::Tls => TunnelRelayTrouble::Tls,
                RelayTrouble::Network => TunnelRelayTrouble::Network,
                RelayTrouble::Other => TunnelRelayTrouble::Other,
            },
            received: status.received,
            link_received: status.link_received,
            dial_timeout_ms: status.dial_timeout_ms,
            reach: match status.reach {
                Reach::None => TunnelReach::None,
                Reach::Local => TunnelReach::Local,
                Reach::Lan => TunnelReach::Lan,
                Reach::Tailnet => TunnelReach::Tailnet,
                Reach::Public => TunnelReach::Public,
            },
        }
    }

    /// After the app was suspended or the network changed: the connection
    /// is made afresh by the next socket, and iroh looks at the network
    /// again.
    pub async fn reconnect(&self) {
        let tunnel = self.tunnel.clone();
        let _ = self.runtime.spawn(async move { tunnel.reconnect().await }).await;
    }

    /// Stops it: the port closes, and so does the connection.
    pub async fn close(&self) {
        let tunnel = self.tunnel.clone();
        let _ = self.runtime.spawn(async move { tunnel.close().await }).await;
    }

    /// Forwards this device's localhost [port] (127.0.0.1 and ::1) to the
    /// same port on the backend's machine, so a sign-in whose browser comes
    /// back to http://127.0.0.1:<port> reaches the program waiting for it
    /// there. Until [ForwardHandle::stop]. The forwarding connection is
    /// dialled first, so a backend from before it fails here (Unsupported).
    pub async fn forward_port(&self, port: u16) -> Result<ForwardHandle, ForwardError> {
        let tunnel = self.tunnel.clone();
        let runtime = self.runtime.clone();
        match runtime.spawn(async move { forward::forward(&tunnel, port).await }).await {
            Ok(Ok(forward)) => Ok(ForwardHandle { forward: Arc::new(forward), runtime }),
            Ok(Err(error)) => Err(error.into()),
            Err(error) => Err(ForwardError { kind: ForwardErrorKind::Failed, message: error.to_string() }),
        }
    }
}

/// A localhost port forwarded to the backend's machine.
#[frb(opaque)]
pub struct ForwardHandle {
    forward: Arc<forward::Forward>,
    runtime: tokio::runtime::Handle,
}

impl ForwardHandle {
    #[frb(sync)]
    pub fn port(&self) -> u16 {
        self.forward.port()
    }

    #[frb(sync)]
    pub fn status(&self) -> ForwardStatus {
        let status = self.forward.status();
        ForwardStatus { connections: status.connections, last_error: status.last_error }
    }

    /// Stops listening; connections being carried finish on their own.
    pub async fn stop(&self) {
        let forward = self.forward.clone();
        let _ = self.runtime.spawn(async move { forward.stop() }).await;
    }
}

pub struct ForwardStatus {
    /// Connections carried so far: above 0 once the browser came back.
    pub connections: u32,
    /// Why the last one couldn't be carried (the backend's sentence, or the
    /// tunnel's); empty while all's well.
    pub last_error: String,
}

/// Why a port can't be forwarded.
#[derive(Debug)]
pub struct ForwardError {
    pub kind: ForwardErrorKind,
    /// Failed: what went wrong, as it is; empty otherwise.
    pub message: String,
}

#[derive(Debug)]
pub enum ForwardErrorKind {
    /// Something on this device already listens on the port (on 127.0.0.1
    /// or ::1): the browser would reach it instead.
    PortTaken,
    /// The backend is from before forwarding.
    Unsupported,
    Failed,
}

impl From<forward::ForwardingFailure> for ForwardError {
    fn from(error: forward::ForwardingFailure) -> ForwardError {
        match error {
            forward::ForwardingFailure::PortTaken => ForwardError { kind: ForwardErrorKind::PortTaken, message: String::new() },
            forward::ForwardingFailure::Unsupported => ForwardError { kind: ForwardErrorKind::Unsupported, message: String::new() },
            forward::ForwardingFailure::Failed(message) => ForwardError { kind: ForwardErrorKind::Failed, message },
        }
    }
}

/// What the tunnel is doing, for the app to show.
pub struct TunnelStatus {
    pub phase: TunnelPhase,
    pub path: TunnelPath,
    /// Why it can't connect, a sentence for the person; empty while all's well.
    pub error: String,
    pub failure: TunnelFailure,
    /// Whether this device reaches its relay now.
    pub relay_connected: bool,
    /// How long its relay took to come up, in ms, from when it was last
    /// lost (or the tunnel's endpoint made); 0 while it's down.
    pub relay_up_ms: u32,
    /// Why its relay can't be reached, roughly, while it can't.
    pub relay_trouble: TunnelRelayTrouble,
    /// Bytes received from the backend so far, as they arrive, wrapping:
    /// while it moves, the link is delivering, even before a whole message
    /// is in (#413).
    pub received: u32,
    /// Bytes the open connection has received at all, QUIC's own traffic
    /// included, wrapping: it moves while the link is alive even when a
    /// backed-off connection holds the data back (#413).
    pub link_received: u32,
    /// How long the next dial has, in ms: it grows after dials that timed
    /// out, and the app's connect window with it (#413).
    pub dial_timeout_ms: u32,
    /// What a direct path crosses, from its address (never the address):
    /// for diagnostics, per route.
    pub reach: TunnelReach,
}

/// What a direct path crosses.
pub enum TunnelReach {
    /// Not direct, or not connected.
    None,
    /// This machine itself.
    Local,
    /// The same network: a private or link-local address.
    Lan,
    /// Tailscale's addresses.
    Tailnet,
    /// Across the internet.
    Public,
}

/// Why this device's relay can't be reached, in a word.
pub enum TunnelRelayTrouble {
    None,
    /// No relay chosen yet: none answered iroh's last measurement.
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

pub enum TunnelPhase {
    Idle,
    Dialing,
    Connected,
    Failed,
}

pub enum TunnelPath {
    None,
    Direct,
    Relayed,
}

pub enum TunnelFailure {
    None,
    Unreachable,
    Refused,
    Offline,
    Other,
}
