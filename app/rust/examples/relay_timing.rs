//! How long a device takes to reach a backend through a relay when no
//! direct path can work: a cold device endpoint (as the app makes at
//! launch, or after a network change), with no IP transport of its own, so
//! everything goes through a relay. A backend on this machine homes on n0's
//! relays or on Claktory's own; the device homes on either; the device
//! dials with the relay a pairing code names (or a stale one, or none, to
//! be looked up). Prints each try and the medians.
//!
//!   cargo run --release --example relay_timing -- [TRIES]
//!
//! It uses the real relays (n0's and relay.claktory.com) and n0's address
//! lookup, like the app and the backend.

use std::time::{Duration, Instant};

use claktory_tunnel::tunnel::ALPN_COMPRESSED;
use iroh::{Endpoint, EndpointAddr, RelayMap, RelayMode, RelayUrl, Watcher, endpoint::presets};

const OURS: &str = "https://relay.claktory.com/";

fn mode(ours: bool) -> RelayMode {
    if ours { RelayMode::Custom(RelayMap::from(OURS.parse::<RelayUrl>().unwrap())) } else { RelayMode::Default }
}

/// A device whose own relay can't be reached: no home relay is ever
/// chosen, as on a phone whose first net report found none.
fn nowhere() -> RelayMode {
    RelayMode::Custom(RelayMap::from("https://relay.claktory.invalid./".parse::<RelayUrl>().unwrap()))
}

async fn backend(ours: bool) -> anyhow::Result<Endpoint> {
    let endpoint = Endpoint::builder(presets::N0).relay_mode(mode(ours)).alpns(vec![ALPN_COMPRESSED.to_vec()]).bind().await?;
    tokio::time::timeout(Duration::from_secs(30), endpoint.online()).await?;
    let server = endpoint.clone();
    tokio::spawn(async move {
        while let Some(incoming) = server.accept().await {
            tokio::spawn(async move {
                if let Ok(connection) = incoming.await {
                    // Held until the device closes it.
                    let _ = connection.closed().await;
                }
            });
        }
    });
    Ok(endpoint)
}

/// One cold dial: (connected after, home relay up after), or None for a
/// timeout.
async fn trial(device: RelayMode, target: EndpointAddr, within: Duration) -> (Option<Duration>, Option<Duration>) {
    let begun = Instant::now();
    let endpoint = Endpoint::builder(presets::N0).relay_mode(device).clear_ip_transports().bind().await.expect("a device endpoint");
    let mut relays = endpoint.home_relay_status();
    let home = tokio::spawn(async move {
        loop {
            if relays.get().iter().any(|relay| relay.is_connected()) {
                return Some(begun.elapsed());
            }
            if relays.updated().await.is_err() {
                return None;
            }
        }
    });
    let dialled = tokio::time::timeout(within, async {
        let connecting = endpoint.connect_with_opts(target, ALPN_COMPRESSED, Default::default()).await?;
        anyhow::Ok(connecting.await?)
    })
    .await;
    let connected = match dialled {
        Ok(Ok(connection)) => {
            let at = begun.elapsed();
            connection.close(0u32.into(), b"");
            Some(at)
        }
        Ok(Err(error)) => {
            eprintln!("    failed: {error:#}");
            None
        }
        Err(_) => None,
    };
    // How long its home relay took, given a few seconds more.
    let home = tokio::time::timeout(Duration::from_secs(5), home).await.ok().and_then(|joined| joined.ok()).flatten();
    let _ = tokio::time::timeout(Duration::from_secs(3), endpoint.close()).await;
    (connected, home)
}

fn show(at: Option<Duration>) -> String {
    at.map(|at| format!("{:.2}s", at.as_secs_f64())).unwrap_or_else(|| "never".into())
}

fn median(mut all: Vec<Option<Duration>>) -> String {
    all.sort_by_key(|at| at.unwrap_or(Duration::MAX));
    show(all[all.len() / 2])
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let tries: usize = std::env::args().nth(1).map(|n| n.parse()).transpose()?.unwrap_or(5);
    let within = Duration::from_secs(20);
    for backend_ours in [false, true] {
        let server = backend(backend_ours).await?;
        let home: Vec<RelayUrl> = server.addr().relay_urls().cloned().collect();
        println!(
            "backend on {} (home {:?})",
            if backend_ours { "ours" } else { "n0" },
            home.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
        let named = EndpointAddr::new(server.id()).with_relay_url(home[0].clone());
        let unnamed = EndpointAddr::new(server.id());
        // A relay the backend isn't on: the code's from before it moved.
        let stale_url: RelayUrl =
            if backend_ours { RelayMode::Default.relay_map().urls::<Vec<RelayUrl>>()[0].clone() } else { OURS.parse()? };
        let stale = EndpointAddr::new(server.id()).with_relay_url(stale_url);
        let cases: [(&str, RelayMode, &EndpointAddr); 6] = [
            ("device on n0, code names the backend's relay", mode(false), &named),
            ("device on ours, code names the backend's relay", mode(true), &named),
            ("device with no home relay, code names the backend's relay", nowhere(), &named),
            ("device on n0, code names no relay (lookup)", mode(false), &unnamed),
            ("device on n0, code names a stale relay", mode(false), &stale),
            ("device on ours, code names a stale relay", mode(true), &stale),
        ];
        for (label, device, target) in cases {
            let mut connected = Vec::new();
            let mut homes = Vec::new();
            let mut line = String::new();
            for _ in 0..tries {
                let (at, home) = trial(device.clone(), target.clone(), within).await;
                line.push_str(&format!(" {}/{}", show(at), show(home)));
                connected.push(at);
                homes.push(home);
            }
            println!("  {label}: connected median {}, home relay median {} [{}]", median(connected), median(homes), line.trim());
        }
        let _ = tokio::time::timeout(Duration::from_secs(3), server.close()).await;
    }
    Ok(())
}
