//! Opens the app's tunnel to a backend, as the app does, with a fresh key, asks it for a socket and prints how
//! its status goes until the backend answers (a direct path that stalls should fall back to the relay within a
//! few seconds). The backend turns the unknown key away, so "refused" means it was reached; nothing is paired
//! or left pending.
//!
//!   cargo run --release --example tunnel_probe -- ENDPOINT_ID [--relay URL]... [--addr IP:PORT]... [--tries N]

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use claktory_tunnel::tunnel::{self, Failure, MakeEndpoint, Tunnel, backend_addr, device_endpoint};
use iroh::SecretKey;
use tokio::{io::AsyncWriteExt, net::TcpStream};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let id = args.next().expect("ENDPOINT_ID");
    let (mut relays, mut addrs, mut tries) = (vec![], vec![], 1u32);
    while let Some(flag) = args.next() {
        let value = args.next().expect("a value");
        match flag.as_str() {
            "--relay" => relays.push(value),
            "--addr" => addrs.push(value),
            "--tries" => tries = value.parse()?,
            other => anyhow::bail!("unknown flag {other}"),
        }
    }
    for _ in 0..tries {
        let key = SecretKey::generate();
        let homes = tunnel::relay_urls(&relays);
        let make: MakeEndpoint = Arc::new(move || {
            let (key, homes) = (key.clone(), homes.clone());
            Box::pin(async move { device_endpoint(key, &homes).await })
        });
        let tunnel = Tunnel::open_with(make, tunnel::REBUILD_AFTER, backend_addr(&id, &relays, &addrs)?).await?;
        let begun = Instant::now();
        let mut socket = TcpStream::connect(("127.0.0.1", tunnel.port())).await?;
        socket.write_all(format!("GET {} HTTP/1.1\r\n", tunnel.path()).as_bytes()).await?;
        let mut last = String::new();
        let mut line = Vec::new();
        while begun.elapsed() < Duration::from_secs(20) {
            let status = tunnel.status();
            let now = format!("{:?} {:?} relay={}", status.phase, status.path, status.relay_connected);
            if now != last {
                line.push(format!("{:.2}s {now}", begun.elapsed().as_secs_f64()));
                last = now;
            }
            if status.failure != Failure::None {
                line.push(format!("{:.2}s {:?}: {}", begun.elapsed().as_secs_f64(), status.failure, status.error));
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        println!("{}", line.join(" | "));
        drop(socket);
        tunnel.close().await;
    }
    Ok(())
}
