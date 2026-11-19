//! A stand-in for the backend's half of the private tunnel, to try the app
//! end to end: an iroh endpoint on `claktory/1` (n0's relays) that forwards
//! each stream to a backend's local WebSocket port. Prints a pairing code.
//!
//!   cargo run --example standin -- 7421 <token>

use iroh::{Endpoint, endpoint::presets};
use tokio::{io::copy, net::TcpStream};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let port: u16 = args.next().unwrap_or_else(|| "7421".into()).parse()?;
    let token = args.next().unwrap_or_else(|| "0".repeat(64));
    let endpoint = Endpoint::builder(presets::N0).alpns(vec![claktory_tunnel::tunnel::ALPN.to_vec()]).bind().await?;
    endpoint.online().await;
    let addr = endpoint.addr();
    let relays: String = addr.relay_urls().map(|url| format!("&r={url}")).collect();
    println!("factory://?t={token}&name=standin&e={}{relays}", endpoint.id());
    while let Some(incoming) = endpoint.accept().await {
        tokio::spawn(async move {
            let Ok(connection) = incoming.await else { return };
            eprintln!("connected: {}", connection.remote_id());
            while let Ok((mut send, mut receive)) = connection.accept_bi().await {
                tokio::spawn(async move {
                    let Ok(socket) = TcpStream::connect(("127.0.0.1", port)).await else { return };
                    let (mut read, mut write) = socket.into_split();
                    let up = async {
                        let _ = copy(&mut receive, &mut write).await;
                    };
                    let down = async {
                        let _ = copy(&mut read, &mut send).await;
                        let _ = send.finish();
                    };
                    tokio::join!(up, down);
                });
            }
        });
    }
    Ok(())
}
