//! The supervisor keeps Factory running through updates and undoes one whose
//! backend doesn't start. Runs the real factoryd in a temporary home on a
//! free port.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use crate::common::{Client, View, apply_request, serves, temp_dir};
use factory_protocol::v1::{self, client_message, server_message::Body};

/// The factoryd under test: this build, or FACTORYD_UNDER_TEST's (a copy
/// signed with the Developer ID, say, to check that the supervisor and its
/// rollback work with signed builds). FACTORYD_UPDATE names the backend the
/// good update brings, by default the same.
fn factoryd(variable: &str) -> PathBuf {
    std::env::var_os(variable).map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_factoryd")), PathBuf::from)
}

/// Stops the supervisor (and so its backend) when the test ends.
struct Supervisor(Child);

impl Drop for Supervisor {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = Command::new("kill").arg(self.0.id().to_string()).status();
        // Its stop event, which it passes on to its backend, as SIGTERM.
        #[cfg(windows)]
        if factoryd::windows::stop::ask_to_stop(self.0.id()).is_err() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn supervise(home: &Path, port: u16) -> Supervisor {
    let log = std::fs::File::create(home.join("factoryd.log")).expect("log");
    let child = Command::new(factoryd("FACTORYD_UNDER_TEST"))
        .arg("supervise")
        .env("FACTORY_HOME", home)
        .env("FACTORY_PORT", port.to_string())
        // The tests' person token for this home, as its backend's.
        .env("FACTORY_TEST_PERSON_TOKEN", factoryd::authority::person_token_for_test(home))
        .stdout(Stdio::from(log.try_clone().expect("log")))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("start supervisor");
    Supervisor(child)
}

/// A hang guard, not a speed check: under load a restart can take many seconds.
const HANG: Duration = Duration::from_secs(90);

/// Connects and reads the state, or None when no backend serves now: one
/// that's going down (it may still accept the connection and then close it)
/// or not up yet.
async fn try_connect(addr: SocketAddr) -> Option<(Client, v1::Workspace)> {
    tokio::net::TcpStream::connect(addr).await.ok()?;
    // The test client panics on a refused or closed connection; here that's
    // "not now".
    let connecting = tokio::spawn(async move {
        let (client, workspace, _) = Client::connect(addr).await;
        (client, workspace)
    });
    connecting.await.ok()
}

/// Connects once the backend is up again, and waits for its state to pass `until`.
async fn reconnect(addr: SocketAddr, until: impl Fn(&v1::Workspace) -> bool) -> (Client, View) {
    let deadline = tokio::time::Instant::now() + HANG;
    while tokio::time::Instant::now() < deadline {
        if let Some((client, workspace)) = try_connect(addr).await
            && until(&workspace)
        {
            return (client, View { workspace, ..View::default() });
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the backend didn't come back as expected");
}

fn publish(home: &Path, version: &str, backend: &[u8]) {
    let release = home.join("releases").join(version);
    std::fs::create_dir_all(&release).expect("mkdir");
    std::fs::write(release.join(factoryd::home::program("factoryd")), backend).expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(release.join(factoryd::home::program("factoryd")), std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    std::fs::write(home.join("releases/latest"), version).expect("write");
}

fn install() -> client_message::Body {
    client_message::Body::InstallUpdate(v1::InstallUpdate { app_path: String::new() })
}

#[tokio::test]
async fn a_backend_update_that_does_not_start_is_undone_and_a_good_one_sticks() {
    let home = temp_dir().join("home");
    std::fs::create_dir_all(home.join("bin")).expect("mkdir");
    let original = std::fs::read(factoryd("FACTORYD_UNDER_TEST")).expect("read factoryd");
    let update = std::fs::read(factoryd("FACTORYD_UPDATE")).expect("read the update");
    // As the installer puts it: `bin/factoryd` (`factoryd.exe` on Windows).
    let installed = factoryd::home::Home::new(home.clone()).backend();
    std::fs::write(&installed, &original).expect("install");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&installed, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr").port();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    serves(addr, factoryd::home::Home::new(home.clone()));
    let _supervisor = supervise(&home, port);
    let (mut client, mut view) = reconnect(addr, |_| true).await;
    assert_eq!(view.workspace.version, "dev");
    // A vault with no key on disk: open only while a backend holds its key,
    // which each hands to the next.
    let recovery = client_message::Body::NewRecoveryKey(v1::NewRecoveryKey { proof: None, ..Default::default() });
    assert!(matches!(apply_request(&mut client, &mut view, recovery).await, Body::RecoveryKey(_)));
    let locked = client_message::Body::SetVaultLockedMode(v1::SetVaultLockedMode { locked_mode: true, proof: None });
    apply_request(&mut client, &mut view, locked).await;
    assert!(!home.join("vault/auto.key").exists());
    let open = |view: &View| view.workspace.vault.as_ref().is_some_and(|vault| vault.unlocked);

    // A release whose backend can't even run: one that exits at once, or on
    // Windows, where a script isn't a program, one that isn't a program.
    let broken: &[u8] = if cfg!(windows) { b"not a program" } else { b"#!/bin/sh\nexit 3\n" };
    publish(&home, "2099.01.01-0000", broken);
    drop((client, view));
    let (mut client, mut view) = reconnect(addr, |workspace| workspace.latest_release == "2099.01.01-0000").await;
    assert!(matches!(apply_request(&mut client, &mut view, install()).await, Body::Ack(_)));

    // The previous backend comes back and says what happened.
    let (_client, view) = reconnect(addr, |workspace| !workspace.update_error.is_empty()).await;
    assert_eq!(view.workspace.version, "dev");
    assert!(
        view.workspace.update_error.starts_with(if cfg!(windows) {
            "Claktory 2099.01.01-0000 didn't start, so you're still on dev. It couldn't be started"
        } else {
            "Claktory 2099.01.01-0000 didn't start, so you're still on dev. It stopped"
        }),
        "{}",
        view.workspace.update_error
    );
    assert_eq!(std::fs::read(&installed).expect("read"), original, "the previous backend is back");
    assert!(!home.join("bin/trial.json").exists());
    assert!(open(&view), "the vault's key came back with it");

    // A release with a working backend passes its trial once it serves the app.
    publish(&home, "2099.01.01-0001", &update);
    let (mut client, mut view) = reconnect(addr, |workspace| workspace.latest_release == "2099.01.01-0001").await;
    assert!(view.workspace.update_error.is_empty(), "a new release clears the old failure");
    assert!(matches!(apply_request(&mut client, &mut view, install()).await, Body::Ack(_)));
    drop((client, view));
    // On trial from the ack on: it passes once the new backend serves an app,
    // which connecting does.
    assert!(home.join("bin/trial.json").exists(), "the update is on trial");
    let deadline = tokio::time::Instant::now() + HANG;
    while home.join("bin/trial.json").exists() {
        assert!(tokio::time::Instant::now() < deadline, "the trial never passed");
        drop(try_connect(addr).await);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (_client, view) = reconnect(addr, |_| true).await;
    assert!(view.workspace.update_error.is_empty());
    assert!(open(&view), "the vault stays open across the update");
    assert!(home.join("bin/factoryd.previous").exists(), "the one before is kept to go back to");
    // Byte for byte, so a signed build's signature stays whole.
    assert_eq!(std::fs::read(&installed).expect("read"), update, "the update is in place");
    assert_eq!(std::fs::read(home.join("bin/factoryd.previous")).expect("read"), original);

    // A client turned away (another user's, say) isn't an app that couldn't
    // be served: with no app, the trial passes when its time is up.
    publish(&home, "2099.01.01-0002", &update);
    let (mut client, mut view) = reconnect(addr, |workspace| workspace.latest_release == "2099.01.01-0002").await;
    assert!(matches!(apply_request(&mut client, &mut view, install()).await, Body::Ack(_)));
    drop((client, view));
    let deadline = tokio::time::Instant::now() + HANG;
    while home.join("bin/trial.json").exists() {
        assert!(tokio::time::Instant::now() < deadline, "the trial never ended");
        if let Ok(tcp) = tokio::net::TcpStream::connect(addr).await
            && let Ok((mut socket, _)) = tokio_tungstenite::client_async(format!("ws://{addr}/"), tcp).await
        {
            let hello = v1::Hello {
                protocol_version: factory_protocol::PROTOCOL_VERSION,
                client_version: "stranger".into(),
                token: "0".repeat(64),
                ..Default::default()
            };
            let message = v1::ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) };
            use futures_util::SinkExt;
            use prost::Message as _;
            let _ = socket.send(tokio_tungstenite::tungstenite::Message::Binary(message.encode_to_vec().into())).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let (_client, view) = reconnect(addr, |_| true).await;
    assert!(view.workspace.update_error.is_empty(), "{}", view.workspace.update_error);
}
