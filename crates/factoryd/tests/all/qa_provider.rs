//! Fugo's guard for qa-model's fake Claude sign-in (#332 tooling): the QA
//! override of Anthropic's address (FACTORY_QA_ANTHROPIC_URL) exists only
//! in a build with the `qa-provider` feature, which scripts/qa.sh builds
//! sandboxes with. The workspace's own build, like a release, has no trace
//! of it and ignores the variable; a QA build takes only an address on this
//! computer, never on a real Claktory's port, and refuses to start
//! otherwise.

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::common::temp_dir;

const VAR: &str = "FACTORY_QA_ANTHROPIC_URL";

/// A factoryd of this build on its own home and `port`, with `url` as the
/// override; its output goes to `dir`/factoryd.log.
fn start(dir: &Path, port: u16, url: &str) -> Child {
    std::fs::create_dir_all(dir.join("user")).expect("user");
    let log = std::fs::File::create(dir.join("factoryd.log")).expect("log");
    Command::new(env!("CARGO_BIN_EXE_factoryd"))
        .env("FACTORY_HOME", dir.join("home"))
        .env("FACTORY_PORT", port.to_string())
        .env("HOME", dir.join("user"))
        .env(VAR, url)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone().expect("log")))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("start factoryd")
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port()
}

/// What factoryd logged once it listens, or once it stopped (with its
/// exit status), within a minute.
fn outcome(dir: &Path, child: &mut Child) -> (Option<std::process::ExitStatus>, String) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let log = std::fs::read_to_string(dir.join("factoryd.log")).unwrap_or_default();
        if let Some(status) = child.try_wait().expect("wait") {
            return (Some(status), log);
        }
        if log.contains("factoryd listening") || Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return (None, log);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(not(feature = "qa-provider"))]
#[test]
fn without_the_qa_feature_there_is_no_override_at_all() {
    let binary = std::fs::read(env!("CARGO_BIN_EXE_factoryd")).expect("binary");
    assert!(!binary.windows(VAR.len()).any(|window| window == VAR.as_bytes()), "this build of factoryd names {VAR}");
    // Set anyway, to an address the guard would refuse: ignored.
    let dir = temp_dir();
    let mut child = start(&dir, free_port(), "https://api.anthropic.com");
    let (stopped, log) = outcome(&dir, &mut child);
    assert!(stopped.is_none() && log.contains("factoryd listening"), "{log}");
    assert!(!log.contains("QA provider override"), "{log}");
}

#[cfg(feature = "qa-provider")]
#[test]
fn a_qa_build_takes_only_an_address_on_this_computer() {
    let dir = temp_dir();
    let mut child = start(&dir, free_port(), "http://127.0.0.1:9");
    let (stopped, log) = outcome(&dir, &mut child);
    assert!(stopped.is_none() && log.contains("QA provider override: http://127.0.0.1:9"), "{log}");

    for (url, port, says) in [
        ("https://api.anthropic.com", free_port(), "only an http(s) address on this computer"),
        ("http://localhost:9", free_port(), "only an http(s) address on this computer"),
        ("http://127.0.0.1:9", 7420, "port 7420, a real Claktory's"),
        ("http://127.0.0.1:9", 7421, "port 7421, a real Claktory's"),
    ] {
        let dir = temp_dir();
        let mut child = start(&dir, port, url);
        let (stopped, log) = outcome(&dir, &mut child);
        assert!(stopped.is_some_and(|status| !status.success()), "{url} on {port} started: {log}");
        assert!(log.contains(says) && log.contains("refused") && !log.contains("factoryd listening"), "{url} on {port}: {log}");
    }
}
