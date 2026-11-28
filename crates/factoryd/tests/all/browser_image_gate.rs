//! The Mac VM gate for a release's browser image (#435 W11): a release's
//! own factoryd starts Claktory's browser from the image its release ships
//! (`browser_host::image`), loading it first when it isn't, and reaches no
//! registry for it.
//!
//! From v0.2.0 only the person opens the browser: a paired device, or the
//! release's own Claktory Dev on this Mac over its socket. So the person
//! starts it there (Agent browser, open example.com), and this test only
//! watches, as a command does, with the tool token: it prints the setup
//! card as it changes, passes once the browser is browsing within 15
//! minutes, and waits for the person to stop it. Run it on a Mac whose
//! `container` can start virtual machines, against the release's factoryd
//! in a throwaway home and port (never the shared dev Claktory's), started
//! with FACTORY_AGENT_BROWSER=1, its Claktory Dev opened with HOME set to
//! that home's parent. Its Claktory Dev is let in only once its code is
//! pinned (a `--dev` release doesn't pin it; until then its socket is
//! refused with "no Dev app is pinned in …"):
//!
//!   codesign -dv --verbose=4 "<the release's Factory Dev.app>" 2>&1 \
//!     | sed -n 's/^CDHash=//p' | head -n 1 >> /tmp/w11v2/.factory-dev/app-pins
//!   chmod 600 /tmp/w11v2/.factory-dev/app-pins
//!
//! Then:
//!
//!   CLAKTORY_GATE_ADDR=127.0.0.1:7611 CLAKTORY_GATE_HOME=/tmp/w11v2/.factory-dev \
//!     cargo test -p factoryd --test all browser_image_gate:: -- --ignored --nocapture
//!
//! then open the browser in Claktory Dev, keep a screenshot of its live
//! view, and stop it. Watch `lsof -nP -i -a -p <factoryd>` and
//! `container`'s processes meanwhile: neither should reach a registry or
//! download anything.

#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{AgentBrowserState as State, ClientMessage, Hello, client_message};

use crate::common::{Client, View};

fn state(view: &View) -> Option<State> {
    view.workspace.agent_browser.as_ref().map(|browser| browser.state())
}

fn setup(view: &View) -> String {
    view.workspace.agent_browser.as_ref().and_then(|browser| browser.setup.as_ref()).map(|setup| setup.problem.clone()).unwrap_or_default()
}

/// Connected as a command is, with the tool token: it reads the Workspace
/// and may open nothing.
async fn connect(addr: std::net::SocketAddr, home: &str) -> (Client, View) {
    let token = std::fs::read_to_string(format!("{home}/tool-token")).expect("the tool token").trim().to_owned();
    let mut client = Client::open(addr).await;
    let hello = Hello { protocol_version: PROTOCOL_VERSION, client_version: "gate".into(), token, ..Default::default() };
    client.send(ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) }).await;
    let (workspace, _) = client.welcomed().await;
    (client, View { workspace, ..View::default() })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn a_release_starts_its_browser_from_the_image_it_ships() {
    let (Ok(addr), Ok(home)) = (std::env::var("CLAKTORY_GATE_ADDR"), std::env::var("CLAKTORY_GATE_HOME")) else {
        eprintln!("no CLAKTORY_GATE_ADDR and CLAKTORY_GATE_HOME: skipped");
        return;
    };
    let addr = addr.parse().expect("an address");
    let (mut client, mut view) = connect(addr, &home).await;
    eprintln!("watching: open the browser in Claktory Dev now (Agent browser, example.com)");
    let started = Instant::now();
    let mut said = (None, String::new());
    let mut seen = |view: &View, started: Instant| {
        let now = (state(view), setup(view));
        if now != said {
            eprintln!("{:>6.1}s {:?} (card: {:?})", started.elapsed().as_secs_f32(), now.0, now.1);
            said = now;
        }
    };
    // Ready within 15 minutes of being opened: the image loaded from the
    // release if it wasn't, and the browser started from it.
    tokio::time::timeout(
        Duration::from_secs(900),
        view.follow(&mut client, |view| {
            seen(view, started);
            state(view) == Some(State::Browsing)
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("browsing within 15 minutes: {:?}", view.workspace.agent_browser));
    eprintln!(
        "{:>6.1}s browsing: keep a screenshot of the live view in Claktory Dev, then stop the browser",
        started.elapsed().as_secs_f32()
    );
    tokio::time::timeout(
        Duration::from_secs(900),
        view.follow(&mut client, |view| {
            seen(view, started);
            !matches!(state(view), Some(State::Browsing | State::Starting))
        }),
    )
    .await
    .unwrap_or_else(|_| panic!("stopped within 15 minutes: {:?}", view.workspace.agent_browser));
    eprintln!("{:>6.1}s stopped", started.elapsed().as_secs_f32());
}
