//! What Claktory's links to other Claktories share over the tunnel's
//! endpoint: dialing a peer by its key on one ALPN, the addresses and relays
//! a peer may point this side at, the length-prefixed JSON each stream
//! carries back, and how long to wait between tries. Pipes
//! (`claktory-pipe/1`, `pipe_link`) and direct messages between home
//! Claktories (`claktory-dm/1`, `dm_link`) each keep their own frames and
//! gate.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use iroh::endpoint::{ConnectError, ConnectingError, Connection, ConnectionError, SendStream};
use iroh::{EndpointAddr, EndpointId, RelayUrl};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::factory::Factory;

/// The longest wait between tries to reach a peer. Long, since each try at
/// a peer that's away sends ~46 KB for nothing (Polpo's pipe-redial.md): a
/// frame from the peer, or something queued for it, wakes the worker anyway.
pub(crate) const MAX_BACKOFF: Duration = Duration::from_secs(15 * 60);
/// The first wait after a try fails, which then doubles.
pub(crate) const FIRST_BACKOFF: Duration = Duration::from_secs(1);

/// Why a dial didn't connect.
#[derive(Debug)]
pub(crate) enum DialError {
    /// The peer's gate turned this Claktory's key away (`turned_away`).
    TurnedAway,
    Failed(String),
}

impl std::fmt::Display for DialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DialError::TurnedAway => f.write_str("it turned the code away"),
            DialError::Failed(why) => f.write_str(why),
        }
    }
}

/// Whether the peer's gate closed `error`'s connection, refusing this key
/// (`remote::REFUSED`).
pub(crate) fn turned_away(error: &ConnectionError) -> bool {
    matches!(error, ConnectionError::ApplicationClosed(close) if close.error_code == crate::remote::REFUSED.into())
}

/// Dials `peer` (its key, hex) on `alpn`, at what it's known by: its key
/// alone finds it through the address lookup when the tunnel has one.
pub(crate) async fn dial(
    factory: &Arc<Factory>,
    peer: &str,
    alpn: &[u8],
    relays: &[String],
    addresses: &[String],
    timeout: Duration,
) -> Result<Connection, DialError> {
    let failed = |why: &str| DialError::Failed(why.to_owned());
    let endpoint = factory.remote_endpoint().ok_or_else(|| failed("remote access isn't running"))?;
    let id: EndpointId = peer.parse().map_err(|_| failed("a bad key"))?;
    let mut addr = EndpointAddr::new(id);
    for relay in relays {
        if let Ok(url) = relay.parse::<RelayUrl>() {
            addr = addr.with_relay_url(url);
        }
    }
    for address in addresses {
        if let Ok(address) = address.parse::<SocketAddr>() {
            addr = addr.with_ip_addr(address);
        }
    }
    let connecting = endpoint.connect(addr, alpn);
    match tokio::time::timeout(timeout, connecting).await {
        Ok(Ok(connection)) => Ok(connection),
        Ok(Err(
            ConnectError::Connection { source, .. }
            | ConnectError::Connecting { source: ConnectingError::ConnectionError { source, .. }, .. },
        )) if turned_away(&source) => Err(DialError::TurnedAway),
        Ok(Err(error)) => Err(DialError::Failed(format!("{error:#}"))),
        Err(_) => Err(failed("no answer in time")),
    }
}

/// The relays a peer may point this Claktory at: https only. The tunnel
/// is end to end, so a relay never reads the traffic; the rule is about
/// where the dial goes: an http relay URL could point it at any host the
/// peer names, this machine's own network included. What the person
/// pasted (an invite code's relays) is taken as it is.
pub(crate) fn peer_relays(relays: Vec<String>) -> Vec<String> {
    relays.into_iter().filter(|relay| relay.parse::<RelayUrl>().is_ok_and(|url| url.scheme() == "https")).collect()
}

/// The addresses a peer may point this Claktory at: never loopback,
/// link-local or unspecified, which would aim its dials at its own
/// machine or network segment. An invite code's addresses, pasted
/// by the person, are taken as they are.
pub(crate) fn peer_addresses(addresses: Vec<String>) -> Vec<String> {
    addresses.into_iter().filter(|address| address.parse::<SocketAddr>().is_ok_and(|address| dialable(address.ip()))).collect()
}

fn dialable(ip: IpAddr) -> bool {
    // An IPv4 address mapped into IPv6 ([::ffff:127.0.0.1]) is judged as
    // its IPv4 self.
    let ip = ip.to_canonical();
    match ip {
        IpAddr::V4(ip) => !(ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast()),
        IpAddr::V6(ip) => !(ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xffc0) == 0xfe80),
    }
}

/// Where this Claktory is reached now, for its peers.
pub(crate) fn here(factory: &Factory) -> (Vec<String>, Vec<String>) {
    match factory.remote_endpoint() {
        Some(endpoint) => {
            let (relays, addresses) = factory.reached_at(&endpoint);
            (relays, addresses.iter().map(ToString::to_string).collect())
        }
        None => (Vec::new(), Vec::new()),
    }
}

/// Writes `value` as JSON after its length (4 bytes, big-endian), and ends
/// the stream: a reply.
pub(crate) async fn write_json<T: Serialize>(send: &mut SendStream, value: &T) -> std::io::Result<()> {
    let body = serde_json::to_vec(value).unwrap_or_default();
    send.write_u32(u32::try_from(body.len()).unwrap_or(u32::MAX)).await?;
    send.write_all(&body).await?;
    send.finish().map_err(std::io::Error::other)?;
    Ok(())
}

/// Sends `frame` on a stream of its own and reads the peer's reply (JSON
/// after its length, at most `max_reply` bytes), within `patience` and a
/// second more for each megabyte sent.
pub(crate) async fn exchange<R: DeserializeOwned>(
    connection: &Connection,
    frame: &[u8],
    patience: Duration,
    max_reply: usize,
) -> Result<R, String> {
    let talk = async {
        let (mut send, mut receive) = connection.open_bi().await.map_err(|error| error.to_string())?;
        send.write_all(frame).await.map_err(|error| error.to_string())?;
        send.finish().map_err(|error| error.to_string())?;
        let length = receive.read_u32().await.map_err(|error| error.to_string())? as usize;
        if length > max_reply {
            return Err("a reply too large".to_owned());
        }
        let mut body = vec![0; length];
        receive.read_exact(&mut body).await.map_err(|error| error.to_string())?;
        serde_json::from_slice(&body).map_err(|error| error.to_string())
    };
    let time = patience + Duration::from_secs(frame.len() as u64 / 1_000_000);
    tokio::time::timeout(time, talk).await.map_err(|_| "no answer in time".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_never_points_a_dial_at_this_machine_or_its_segment() {
        let given = ["127.0.0.1:1", "[::1]:1", "169.254.1.1:1", "0.0.0.0:1", "[fe80::1]:1", "[::ffff:127.0.0.1]:1", "203.0.113.5:7"];
        assert_eq!(peer_addresses(given.map(str::to_owned).to_vec()), ["203.0.113.5:7"]);
        let relays = ["http://10.0.0.1/", "https://relay.example.com/", "not a url"];
        assert_eq!(peer_relays(relays.map(str::to_owned).to_vec()), ["https://relay.example.com/"]);
    }
}
