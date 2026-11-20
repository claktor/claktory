//! Waiting for the network when Claktory starts with the computer: at login
//! the backend can start before the Mac has a network, and the first model
//! calls and usage reads would fail with nothing wrong but the timing.

use std::time::Duration;

use tokio::sync::OnceCell;

/// The longest Claktory waits for a network at start; after that it goes on
/// without (a computer that's offline on purpose).
const WITHIN: Duration = Duration::from_secs(60);
/// How often it looks again.
const EVERY: Duration = Duration::from_secs(1);
/// Names whose resolving says the network is up: the providers' own.
const PROBES: [&str; 3] = ["api.anthropic.com:443", "chatgpt.com:443", "github.com:443"];

static READY: OnceCell<()> = OnceCell::const_new();

/// Returns once a name resolves, or after a minute. Only a supervised
/// backend waits (one started with the computer, or by an update); tests
/// and a backend run by hand go on at once. Every caller waits for the same
/// first look.
pub(crate) async fn ready() {
    if std::env::var_os(crate::update::SUPERVISED).is_none() {
        return;
    }
    READY.get_or_init(|| wait(WITHIN)).await;
}

async fn wait(within: Duration) {
    let started = tokio::time::Instant::now();
    let mut said = false;
    loop {
        if resolves().await {
            if said {
                tracing::info!(waited_ms = started.elapsed().as_millis() as u64, "the network is up");
            }
            return;
        }
        if started.elapsed() >= within {
            tracing::warn!("no network after {}s; going on without it", within.as_secs());
            return;
        }
        if !said {
            tracing::info!("waiting for the network before agents carry on");
            said = true;
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// Whether any of the probes resolves, within a couple of seconds.
async fn resolves() -> bool {
    for probe in PROBES {
        let looked_up = tokio::time::timeout(Duration::from_secs(2), tokio::net::lookup_host(probe)).await;
        if let Ok(Ok(mut addresses)) = looked_up
            && addresses.next().is_some()
        {
            return true;
        }
    }
    false
}
