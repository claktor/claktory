//! Claktory's browser on a Mac (#435 P6, Fugo 38515): a runner that won't
//! end leaves no VM behind. Its stand-in ignores its stdin's end and
//! SIGTERM, so only a kill ends `container run`, which leaves its VM
//! running unless Claktory kills that too. Run by hand, with `container`'s
//! system running and a small image pulled (`container image pull
//! docker.io/library/alpine:3.20`):
//!
//!   CLAKTORY_TEST_STUB_IMAGE=docker.io/library/alpine:3.20 cargo test \
//!     -p factoryd --test agent_browser_mac_stuck -- --ignored --nocapture
//!
//! Its VMs and volume are `claktory-browser-test`, never a Claktory's.
//! A file of its own: it turns Claktory's browser on for the process.

#[path = "common/mod.rs"]
mod common;

use std::time::Duration;

use factory_protocol::v1::{self, client_message::Body as Request};

use common::{Client, start, temp_dir};

/// The test's VMs running or stopping now.
fn vms() -> Vec<String> {
    let listed = std::process::Command::new("container").args(["list", "--all", "--quiet"]).output().expect("container");
    String::from_utf8_lossy(&listed.stdout).lines().filter(|id| id.starts_with("claktory-browser-test-")).map(str::to_owned).collect()
}

/// Waits until a new connection sees the browser off.
async fn off(addr: std::net::SocketAddr) {
    let started = std::time::Instant::now();
    loop {
        let (_, workspace, _) = Client::connect(addr).await;
        let state = workspace.agent_browser.as_ref().map(|browser| browser.state());
        if state == Some(v1::AgentBrowserState::Off) {
            eprintln!("off after {:?}", started.elapsed());
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "off: {:?}", workspace.agent_browser);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Waits until `wanted` holds of the test's VMs, checking there's never
/// more than one meanwhile.
async fn until(what: &str, wanted: impl Fn(&[String]) -> bool) -> Vec<String> {
    let started = std::time::Instant::now();
    loop {
        let now = vms();
        assert!(now.len() <= 1, "one VM at a time: {now:?}");
        if wanted(&now) {
            eprintln!("{what} after {:?}: {now:?}", started.elapsed());
            return now;
        }
        assert!(started.elapsed() < Duration::from_secs(90), "{what}: {now:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn a_runner_that_wont_end_leaves_no_vm() {
    let Ok(image) = std::env::var("CLAKTORY_TEST_STUB_IMAGE") else {
        eprintln!("no CLAKTORY_TEST_STUB_IMAGE: skipped");
        return;
    };
    factoryd::browser_host::enable_for_test();
    factoryd::browser_host::mac_image_for_test(image);
    factoryd::browser_host::mac_run_for_test(
        vec!["--entrypoint".into(), "/bin/sh".into()],
        vec!["-c".into(), "trap '' TERM INT HUP; while :; do sleep 1; done".into()],
    );
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    std::fs::write(dir.join("home/config.json"), r#"{"browser": {"live_view": true}}"#).expect("config");
    let (addr, _factory) = start(&dir).await;

    // A leftover of this kind, as a factoryd killed mid-run leaves: gone
    // before the first starts.
    for id in vms() {
        let _ = std::process::Command::new("container").args(["delete", "--force", &id]).output();
    }
    let leftover = "claktory-browser-test-1eff";
    let ran = std::process::Command::new("container")
        .args(["run", "-d", "--rm", "--name", leftover, "--label", "claktory.browser=test", "--entrypoint", "/bin/sh"])
        .arg(std::env::var("CLAKTORY_TEST_STUB_IMAGE").unwrap())
        .args(["-c", "while :; do sleep 1; done"])
        .output()
        .expect("container");
    assert!(ran.status.success(), "{}", String::from_utf8_lossy(&ran.stderr));
    assert_eq!(vms(), [leftover]);

    let (mut client, _, _) = Client::connect(addr).await;
    let open = || Request::OpenAgentBrowser(v1::OpenAgentBrowser { url: "https://example.com/".into(), ..Default::default() });
    client.ok(open()).await;
    let first = until("the first VM", |now| now.len() == 1 && now[0] != leftover).await;

    // Stopped: off only once its VM has gone, though its runner ignored
    // its stdin's end and SIGTERM, and `container run` was killed.
    client.ok(Request::StopAgentBrowser(v1::StopAgentBrowser {})).await;
    off(addr).await;
    assert!(vms().is_empty(), "off, and its VM gone: {:?}", vms());

    // Opened again: one VM, a new one.
    client.ok(open()).await;
    let second = until("the second VM", |now| now.len() == 1).await;
    assert_ne!(second, first);
    client.ok(Request::StopAgentBrowser(v1::StopAgentBrowser {})).await;
    off(addr).await;
    assert!(vms().is_empty(), "off, and its VM gone: {:?}", vms());
    let _ = std::process::Command::new("container").args(["volume", "delete", "claktory-browser-test"]).output();
}
