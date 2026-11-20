//! The private tunnel, for Dart: a device key, and a tunnel to a backend
//! that the app reaches at ws://127.0.0.1:<port>.

use std::sync::{Arc, LazyLock};

use flutter_rust_bridge::frb;

use crate::forward;
use crate::tunnel::{self, Failure, Path, Phase};

/// The tunnel's own runtime: iroh's tasks outlive any one call from Dart.
static RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("tunnel").enable_all().build().expect("the tunnel's runtime")
});

/// A tunnel to one backend.
#[frb(opaque)]
pub struct TunnelHandle {
    tunnel: Arc<tunnel::Tunnel>,
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
    RUNTIME
        .spawn(async move {
            let key = tunnel::parse_key(&device_key)?;
            let backend = tunnel::backend_addr(&endpoint_id, &relays, &addresses)?;
            let endpoint = tunnel::device_endpoint(key).await?;
            let tunnel = tunnel::Tunnel::open(endpoint, backend).await?;
            Ok(TunnelHandle { tunnel: Arc::new(tunnel) })
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
    /// Where the app connects: ws://127.0.0.1:<port>.
    #[frb(sync)]
    pub fn port(&self) -> u16 {
        self.tunnel.port()
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
        }
    }

    /// After the app was suspended or the network changed: the connection
    /// is made afresh by the next socket, and iroh looks at the network
    /// again.
    pub async fn reconnect(&self) {
        let tunnel = self.tunnel.clone();
        let _ = RUNTIME.spawn(async move { tunnel.reconnect().await }).await;
    }

    /// Stops it: the port closes, and so does the connection.
    pub async fn close(&self) {
        let tunnel = self.tunnel.clone();
        let _ = RUNTIME.spawn(async move { tunnel.close().await }).await;
    }

    /// Forwards this device's localhost [port] (127.0.0.1 and ::1) to the
    /// same port on the backend's machine, so a sign-in whose browser comes
    /// back to http://127.0.0.1:<port> reaches the program waiting for it
    /// there. Until [ForwardHandle::stop]. The forwarding connection is
    /// dialled first, so a backend from before it fails here (Unsupported).
    pub async fn forward_port(&self, port: u16) -> Result<ForwardHandle, ForwardError> {
        let tunnel = self.tunnel.clone();
        match RUNTIME.spawn(async move { forward::forward(&tunnel, port).await }).await {
            Ok(Ok(forward)) => Ok(ForwardHandle { forward: Arc::new(forward) }),
            Ok(Err(error)) => Err(error.into()),
            Err(error) => Err(ForwardError { kind: ForwardErrorKind::Failed, message: error.to_string() }),
        }
    }
}

/// A localhost port forwarded to the backend's machine.
#[frb(opaque)]
pub struct ForwardHandle {
    forward: Arc<forward::Forward>,
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
        let _ = RUNTIME.spawn(async move { forward.stop() }).await;
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
