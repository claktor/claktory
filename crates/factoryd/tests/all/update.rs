//! Releases published into the Factory's home are offered as updates.

use crate::common::{Client, View, apply_request, start, temp_dir};
use factory_protocol::v1::{self, ErrorCode, client_message, server_message::Body};

fn install(app_path: &str) -> client_message::Body {
    client_message::Body::InstallUpdate(v1::InstallUpdate { app_path: app_path.into() })
}

#[tokio::test]
async fn a_published_release_is_offered_once_it_is_complete() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.version, "dev", "test builds aren't stamped");
    assert_eq!(workspace.latest_release, "");
    let mut view = View::default();
    match apply_request(&mut client, &mut view, install("")).await {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::InvalidRequest),
        other => panic!("nothing to install, got {other:?}"),
    }

    // Named before its files are there, a release isn't offered.
    let releases = dir.join("home/releases");
    std::fs::create_dir_all(releases.join("2099.01.01-0000")).expect("mkdir");
    std::fs::write(releases.join("latest"), "2099.01.01-0000\n").expect("write");
    // Then the backend lands and the name is written again, as release.sh does.
    std::fs::write(releases.join("2099.01.01-0000/factoryd"), "binary").expect("write");
    std::fs::write(releases.join("latest"), "2099.01.01-0000").expect("write");
    view.follow(&mut client, |view| view.workspace.latest_release == "2099.01.01-0000").await;

    // An app that isn't on this computer can't be swapped, and nothing restarts.
    match apply_request(&mut client, &mut view, install("/nowhere/Factory.app")).await {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::InvalidRequest),
        other => panic!("expected an error, got {other:?}"),
    }
    assert!(!dir.join("home/bin/factoryd").exists());
}

#[tokio::test]
async fn a_release_that_only_changed_the_app_leaves_the_backend_running() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    let release = dir.join("home/releases/2099.01.01-0000");
    std::fs::create_dir_all(&release).expect("mkdir");
    std::fs::write(release.join("factoryd"), "binary").expect("write");
    // The backend in it is the one running (test builds are "dev").
    std::fs::write(release.join("backend-version"), "dev\n").expect("write");
    std::fs::write(dir.join("home/releases/latest"), "2099.01.01-0000").expect("write");
    view.follow(&mut client, |view| view.workspace.latest_release == "2099.01.01-0000").await;
    assert_eq!(view.workspace.latest_backend, "dev");

    match apply_request(&mut client, &mut view, install("")).await {
        Body::Ack(_) => {}
        other => panic!("expected an ack, got {other:?}"),
    }
    assert!(!dir.join("home/bin/factoryd").exists(), "the backend wasn't replaced or restarted");
}
