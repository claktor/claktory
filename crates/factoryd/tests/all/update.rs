//! Releases published into the Factory's home are offered as updates.

use crate::common::{Client, View, apply_request, start, temp_dir};
use factory_protocol::v1::{self, ErrorCode, client_message, server_message::Body};

/// The backend's file name in a release: `factoryd.exe` on Windows.
fn backend() -> String {
    factoryd::home::program("factoryd")
}

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
    std::fs::write(releases.join("2099.01.01-0000").join(backend()), "binary").expect("write");
    std::fs::write(releases.join("latest"), "2099.01.01-0000").expect("write");
    view.follow(&mut client, |view| view.workspace.latest_release == "2099.01.01-0000").await;

    // An app that isn't on this computer can't be swapped, and nothing restarts.
    match apply_request(&mut client, &mut view, install("/nowhere/Factory.app")).await {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::InvalidRequest),
        other => panic!("expected an error, got {other:?}"),
    }
    assert!(!dir.join("home/bin").join(backend()).exists());
}

#[tokio::test]
async fn a_release_that_only_changed_the_app_leaves_the_backend_running() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    let release = dir.join("home/releases/2099.01.01-0000");
    std::fs::create_dir_all(&release).expect("mkdir");
    std::fs::write(release.join(backend()), "binary").expect("write");
    // The backend in it is the one running (test builds are "dev").
    std::fs::write(release.join("backend-version"), "dev\n").expect("write");
    std::fs::write(dir.join("home/releases/latest"), "2099.01.01-0000").expect("write");
    view.follow(&mut client, |view| view.workspace.latest_release == "2099.01.01-0000").await;
    assert_eq!(view.workspace.latest_backend, "dev");

    match apply_request(&mut client, &mut view, install("")).await {
        Body::Ack(_) => {}
        other => panic!("expected an ack, got {other:?}"),
    }
    assert!(!dir.join("home/bin").join(backend()).exists(), "the backend wasn't replaced or restarted");
}

/// An update to a backend on another line names the paired devices whose
/// app it would leave behind, by the app version each last said.
#[tokio::test]
async fn an_update_names_the_devices_whose_app_it_would_leave_behind() {
    let dir = temp_dir();
    let home = crate::common::home(&dir);
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let (_, phone) = factoryd::remote::pair_offline(&home, "Phone").expect("pair");
    let (_, mac) = factoryd::remote::pair_offline(&home, "Mac").expect("pair");
    factoryd::remote::pair_offline(&home, "Tablet").expect("pair");
    let path = dir.join("home/remote/devices.json");
    let mut devices: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("devices")).expect("json");
    for device in devices.as_array_mut().expect("a list") {
        let version = match device["id"].as_str() {
            Some(id) if id == phone.id => "v0.1.19",
            Some(id) if id == mac.id => "v9.0.3",
            _ => continue,
        };
        device["app_version"] = version.into();
    }
    std::fs::write(&path, devices.to_string()).expect("devices");
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let behind = |view: &View| -> Vec<(String, String, bool)> {
        let mut devices: Vec<(String, String, bool)> = view.workspace.remote.as_ref().map_or_else(Vec::new, |remote| {
            remote.devices.iter().map(|device| (device.name.clone(), device.app_version.clone(), device.update_leaves_behind)).collect()
        });
        devices.sort();
        devices
    };
    assert_eq!(behind(&view).iter().filter(|device| device.2).count(), 0, "no update on offer");

    let release = dir.join("home/releases/v9.0.4");
    std::fs::create_dir_all(&release).expect("mkdir");
    std::fs::write(release.join(backend()), "binary").expect("write");
    std::fs::write(dir.join("home/releases/latest"), "v9.0.4").expect("write");
    view.follow(&mut client, |view| view.workspace.latest_release == "v9.0.4").await;
    let row = |name: &str, version: &str, left: bool| (name.to_owned(), version.to_owned(), left);
    // The phone's app is on 0.1; the Mac's on 9.0 already; the tablet never said.
    assert_eq!(behind(&view), [row("Mac", "v9.0.3", false), row("Phone", "v0.1.19", true), row("Tablet", "", false)]);
}
