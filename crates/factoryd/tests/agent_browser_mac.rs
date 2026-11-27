//! Claktory's browser on a Mac (#435 P6): the runner in a VM of Apple's
//! container tool, from a browser image. Run by hand, with an image loaded
//! (`container image load`) and its pinned reference:
//!
//!   CLAKTORY_TEST_BROWSER_IMAGE=NAME@sha256:… cargo test -p factoryd \
//!     --test agent_browser_mac -- --ignored --nocapture
//!
//! It opens the browser at a few pages, keeps a frame of each in
//! $TMPDIR/claktory-browser-mac/ to look at, stops it and deletes its data.
//! Its VMs and volume are `claktory-browser-test`, never a Claktory's.
//! A file of its own: it turns Claktory's browser on for the process.
//!
//! Claktory's browser isn't on Windows.
#![cfg(unix)]

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

use factory_protocol::v1::{self, AgentBrowserState as State, client_message::Body as Request, server_message::Body};

use common::{Client, View, start, temp_dir};

fn state(view: &View) -> Option<State> {
    view.workspace.agent_browser.as_ref().map(|browser| browser.state())
}

/// Waits until a new connection sees the browser off.
async fn off(addr: std::net::SocketAddr) {
    let started = std::time::Instant::now();
    loop {
        let (_, workspace, _) = Client::connect(addr).await;
        let view = View { workspace, ..View::default() };
        if state(&view) == Some(State::Off) {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "off: {:?}", view.workspace.agent_browser);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The test's VMs running now.
fn vms() -> String {
    let listed = std::process::Command::new("container").args(["list", "--quiet"]).output().expect("container");
    String::from_utf8_lossy(&listed.stdout).lines().filter(|id| id.starts_with("claktory-browser-test-")).collect::<Vec<_>>().join(" ")
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn the_browser_runs_in_a_vm_on_a_mac() {
    let Ok(image) = std::env::var("CLAKTORY_TEST_BROWSER_IMAGE") else {
        eprintln!("no CLAKTORY_TEST_BROWSER_IMAGE: skipped");
        return;
    };
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::mac_image_for_test(image);
    let frames = std::env::temp_dir().join("claktory-browser-mac");
    std::fs::create_dir_all(&frames).expect("frames");
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::write(dir.join("home/config.json"), r#"{"browser": {"live_view": true}}"#).expect("config");
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    eprintln!("setup: {:?}", view.workspace.agent_browser);

    let open = |url: &str| Request::OpenAgentBrowser(v1::OpenAgentBrowser { url: url.to_owned(), device_time_zone: "Europe/Rome".into(), ..Default::default() });
    let started = std::time::Instant::now();
    client.ok(open("https://example.com/")).await;
    tokio::time::timeout(Duration::from_secs(300), view.follow(&mut client, |view| state(view) == Some(State::Browsing)))
        .await
        .unwrap_or_else(|_| panic!("it opens: {:?}", view.workspace.agent_browser));
    eprintln!("browsing after {:?}; VMs: {}", started.elapsed(), vms());
    client.ok(Request::WatchAgentBrowser(v1::WatchAgentBrowser { on: true, max_width: 1280, ..Default::default() })).await;

    for (name, url) in [
        ("example", None),
        ("webgl", Some("https://get.webgl.org/")),
        ("time-zone", Some("https://webbrowsertools.com/timezone/")),
    ] {
        if let Some(url) = url {
            client.ok(open(url)).await;
        }
        // The page's frames for a while: the last is kept.
        let mut last = None;
        let until = tokio::time::Instant::now() + Duration::from_secs(if url.is_some() { 25 } else { 8 });
        while let Ok(frame) = tokio::time::timeout_at(
            until,
            client.wait_for(|body| if let Body::AgentBrowserFrame(frame) = body { Some(frame.clone()) } else { None }),
        )
        .await
        {
            client.ok(Request::AgentBrowserFrameAck(v1::AgentBrowserFrameAck { seq: frame.seq })).await;
            last = Some(frame);
        }
        let frame = last.unwrap_or_else(|| panic!("frames of {name}"));
        let path = frames.join(format!("{name}.jpg"));
        std::fs::write(&path, &frame.jpeg).expect("frame");
        eprintln!("{name}: {}x{} of {}x{}, {} ({})", frame.width, frame.height, frame.screen_width, frame.screen_height, path.display(), frame.origin);
    }

    client.ok(Request::StopAgentBrowser(v1::StopAgentBrowser::default())).await;
    let stopping = std::time::Instant::now();
    off(addr).await;
    while !vms().is_empty() {
        assert!(stopping.elapsed() < Duration::from_secs(30), "the VM ended with it: {}", vms());
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    eprintln!("stopped, VM gone, after {:?}", stopping.elapsed());

    // Its data deleted: started again only to delete the profile, then off.
    let (mut other, _, _) = Client::connect(addr).await;
    other.ok(Request::DeleteAgentBrowserData(v1::DeleteAgentBrowserData::default())).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    off(addr).await;
    eprintln!("deleted; VMs: {:?}", vms());
    let _ = std::process::Command::new("container").args(["volume", "delete", "claktory-browser-test"]).output();
}
