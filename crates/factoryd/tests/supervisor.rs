//! The supervisor keeps Factory running through updates and undoes one whose
//! backend doesn't start. Runs the real factoryd in a temporary home on a
//! free port.

mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::{Client, View, apply_request, temp_dir};
use factory_protocol::v1::{self, client_message, server_message::Body};

const FACTORYD: &str = env!("CARGO_BIN_EXE_factoryd");

/// Stops the supervisor (and so its backend) when the test ends.
struct Supervisor(Child);

impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = Command::new("kill").arg(self.0.id().to_string()).status();
        let _ = self.0.wait();
    }
}

fn supervise(home: &Path, port: u16) -> Supervisor {
    let log = std::fs::File::create(home.join("factoryd.log")).expect("log");
    let child = Command::new(FACTORYD)
        .arg("supervise")
        .env("FACTORY_HOME", home)
        .env("FACTORY_PORT", port.to_string())
        .stdout(Stdio::from(log.try_clone().expect("log")))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("start supervisor");
    Supervisor(child)
}

/// Connects once the backend is up again, and waits for its state to pass `until`.
async fn reconnect(addr: SocketAddr, until: impl Fn(&v1::Workspace) -> bool) -> (Client, View) {
    // A hang guard, not a speed check: under load a restart can take many seconds.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    while tokio::time::Instant::now() < deadline {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            let (client, workspace, _) = Client::connect(addr).await;
            if until(&workspace) {
                return (client, View { workspace, ..View::default() });
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the backend didn't come back as expected");
}

fn publish(home: &Path, version: &str, backend: &[u8]) {
    let release = home.join("releases").join(version);
    std::fs::create_dir_all(&release).expect("mkdir");
    std::fs::write(release.join("factoryd"), backend).expect("write");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(release.join("factoryd"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
    std::fs::write(home.join("releases/latest"), version).expect("write");
}

fn install() -> client_message::Body {
    client_message::Body::InstallUpdate(v1::InstallUpdate { app_path: String::new() })
}

#[tokio::test]
async fn a_backend_update_that_does_not_start_is_undone_and_a_good_one_sticks() {
    let home = temp_dir().join("home");
    std::fs::create_dir_all(home.join("bin")).expect("mkdir");
    let original = std::fs::read(FACTORYD).expect("read factoryd");
    std::fs::write(home.join("bin/factoryd"), &original).expect("install");
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(home.join("bin/factoryd"), std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let port = std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let _supervisor = supervise(&home, port);
    let (mut client, mut view) = reconnect(addr, |_| true).await;
    assert_eq!(view.workspace.version, "dev");

    // A release whose backend can't even run.
    publish(&home, "2099.01.01-0000", b"#!/bin/sh\nexit 3\n");
    view.follow(&mut client, |view| view.workspace.latest_release == "2099.01.01-0000").await;
    assert!(matches!(apply_request(&mut client, &mut view, install()).await, Body::Ack(_)));

    // The previous backend comes back and says what happened.
    let (_client, view) = reconnect(addr, |workspace| !workspace.update_error.is_empty()).await;
    assert_eq!(view.workspace.version, "dev");
    assert!(
        view.workspace.update_error.starts_with("Factory 2099.01.01-0000 didn't start, so Factory went back to dev. It stopped"),
        "{}",
        view.workspace.update_error
    );
    assert_eq!(std::fs::read(home.join("bin/factoryd")).expect("read"), original, "the previous backend is back");
    assert!(!home.join("bin/trial.json").exists());

    // A release with a working backend passes its trial once it serves the app.
    publish(&home, "2099.01.01-0001", &original);
    let (mut client, mut view) = reconnect(addr, |workspace| workspace.latest_release == "2099.01.01-0001").await;
    assert!(view.workspace.update_error.is_empty(), "a new release clears the old failure");
    assert!(matches!(apply_request(&mut client, &mut view, install()).await, Body::Ack(_)));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (_client, view) = reconnect(addr, |_| true).await;
    for _ in 0..1200 {
        if !home.join("bin/trial.json").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!home.join("bin/trial.json").exists(), "the trial passed");
    assert!(view.workspace.update_error.is_empty());
    assert!(home.join("bin/factoryd.previous").exists(), "the one before is kept to go back to");
}
