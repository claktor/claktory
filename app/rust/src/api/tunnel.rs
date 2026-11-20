//! The private tunnel, for Dart: a device key, and a tunnel to a backend
//! that the app reaches at ws://127.0.0.1:<port>.

use std::sync::{Arc, LazyLock};

use flutter_rust_bridge::frb;

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
pub async fn open_tunnel(device_key: String, endpoint_id: String, relays: Vec<String>, addresses: Vec<String>) -> anyhow::Result<TunnelHandle> {
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
