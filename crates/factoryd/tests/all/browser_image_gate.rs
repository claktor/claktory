//! The Mac VM gate for a release's browser image (#435 W11): a release's
//! own factoryd, running apart, starts Claktory's browser from the image
//! its release ships (`browser_host::image`), loading it first when it
//! isn't. Run by hand against a factoryd started with
//! FACTORY_AGENT_BROWSER=1, FACTORY_HOME and FACTORY_PORT:
//!
//!   CLAKTORY_GATE_ADDR=127.0.0.1:7590 CLAKTORY_GATE_HOME=/path/to/home \
//!     cargo test -p factoryd --test all browser_image_gate:: -- --ignored --nocapture
//!
//! It prints the setup card as it changes, opens example.com, keeps a frame
//! in $TMPDIR/claktory-browser-gate.jpg, and stops the browser. Watch
//! `lsof -nP -i -a -p <factoryd>` and `container`'s processes meanwhile:
//! neither should reach a registry or download anything.

#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, AgentBrowserState as State, ClientMessage, Hello, client_message, server_message::Body};

use crate::common::{Client, View, code_token};

fn state(view: &View) -> Option<State> {
    view.workspace.agent_browser.as_ref().map(|browser| browser.state())
}

fn setup(view: &View) -> String {
    view.workspace.agent_browser.as_ref().and_then(|browser| browser.setup.as_ref()).map(|setup| setup.problem.clone()).unwrap_or_default()
}

async fn connect(addr: std::net::SocketAddr, home: &str) -> (Client, View) {
    let code = std::fs::read_to_string(format!("{home}/remote/local-pairing")).expect("the local pairing code");
    let mut client = Client::open(addr).await;
    let hello = Hello { protocol_version: PROTOCOL_VERSION, client_version: "gate".into(), token: code_token(&code), ..Default::default() };
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
    let started = Instant::now();
    // Opened again until it's let start: refused meanwhile with what the
    // setup card says (the image getting ready, or why it can't).
    let open = v1::OpenAgentBrowser { url: "https://example.com/".into(), device_time_zone: "Europe/London".into(), ..Default::default() };
    let mut said = String::new();
    let (mut client, mut view) = loop {
        let (mut client, view) = connect(addr, &home).await;
        let now = match client.request(client_message::Body::OpenAgentBrowser(open.clone())).await.body {
            Body::Ack(_) => break (client, view),
            Body::Error(error) => error.message,
            other => panic!("{other:?}"),
        };
        if now != said {
            eprintln!("{:>6.1}s refused: {now:?} (card: {:?})", started.elapsed().as_secs_f32(), setup(&view));
            said = now;
        }
        assert!(started.elapsed() < Duration::from_secs(900), "ready within 15 minutes");
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    eprintln!("{:>6.1}s let start", started.elapsed().as_secs_f32());
    tokio::time::timeout(Duration::from_secs(300), view.follow(&mut client, |view| state(view) == Some(State::Browsing)))
        .await
        .unwrap_or_else(|_| panic!("it opens: {:?}", view.workspace.agent_browser));
    eprintln!("{:>6.1}s browsing", started.elapsed().as_secs_f32());
    client.ok(client_message::Body::WatchAgentBrowser(v1::WatchAgentBrowser { on: true, max_width: 1280, ..Default::default() })).await;
    // The page's frames for a while: the last is kept.
    let mut last = None;
    let until = tokio::time::Instant::now() + Duration::from_secs(10);
    while let Ok(frame) = tokio::time::timeout_at(
        until,
        client.wait_for(|body| if let Body::AgentBrowserFrame(frame) = body { Some(frame.clone()) } else { None }),
    )
    .await
    {
        client.ok(client_message::Body::AgentBrowserFrameAck(v1::AgentBrowserFrameAck { seq: frame.seq })).await;
        last = Some(frame);
    }
    let frame = last.expect("a frame");
    let path = std::env::temp_dir().join("claktory-browser-gate.jpg");
    std::fs::write(&path, &frame.jpeg).expect("frame");
    eprintln!("frame {}x{}: {}", frame.width, frame.height, path.display());
    client.ok(client_message::Body::StopAgentBrowser(v1::StopAgentBrowser::default())).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
}
