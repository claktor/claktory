//! The app dock (C5): the person pins installed apps and orders them, the
//! same on every device; installing an app with views the first time pins
//! it last, and one the person unpinned stays unpinned through updates.

use ed25519_dalek::SigningKey;
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};

use crate::app_data::Stand;
use crate::apps::clapp;
use crate::common::{Client, home_of, last_workspace, start, temp_dir};

fn manifest(id: &str, views: bool) -> String {
    let mut text = format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\npublisher = \"claktory\"\n");
    if views {
        text.push_str("world = \"claktory:app@1.1\"\n\n[[views]]\nid = \"main\"\ntitle = \"Main\"\n");
    } else {
        text.push_str("world = \"claktory:app@1\"\n");
    }
    text
}

/// Installs `id`; a device connecting then sees `dock`.
async fn install(client: &mut Client, addr: std::net::SocketAddr, id: &str, views: bool, dock: &[&str]) {
    let bundle = clapp(&manifest(id, views), b"\0asm\x01\0\0\0 pins", &SigningKey::from_bytes(&[12; 32]));
    let install = |approve: bool, sha256: &str| {
        Request::InstallLocalApp(v1::InstallLocalApp { path: String::new(), approve, sha256: sha256.into(), content: bundle.clone() })
    };
    let Body::AppPermissions(permissions) = client.request(install(false, "")).await.body else { panic!("permissions") };
    client.ok(install(true, &permissions.sha256)).await;
    let (_, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.pinned_apps, dock, "after installing {id}");
}

async fn pin(client: &mut Client, app: &str, pinned: bool, position: i32) -> Vec<String> {
    let events = client.ok(Request::PinApp(v1::PinApp { app_id: app.into(), pinned, position })).await;
    last_workspace(&events).expect("the workspace, before the answer").pinned_apps.clone()
}

#[tokio::test]
async fn the_dock_keeps_the_persons_pins_in_their_order() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let stand = Stand::default();
    factory.set_apps_host(stand.launcher());
    let (mut mac, workspace, _) = Client::connect(addr).await;
    assert!(workspace.pinned_apps.is_empty());

    // Apps with views go last as they're installed; one without doesn't.
    install(&mut mac, addr, "charts", true, &["charts"]).await;
    install(&mut mac, addr, "notes", true, &["charts", "notes"]).await;
    install(&mut mac, addr, "telegram", false, &["charts", "notes"]).await;

    // The person pins, moves and unpins; another device sees the same.
    assert_eq!(pin(&mut mac, "telegram", true, 0).await, ["telegram", "charts", "notes"]);
    assert_eq!(pin(&mut mac, "notes", true, 1).await, ["telegram", "notes", "charts"]);
    assert_eq!(pin(&mut mac, "telegram", true, -1).await, ["notes", "charts", "telegram"]);
    assert_eq!(pin(&mut mac, "notes", true, 99).await, ["charts", "telegram", "notes"]);
    assert_eq!(pin(&mut mac, "charts", false, 0).await, ["telegram", "notes"]);
    let (_phone, on_phone, _) = Client::connect(addr).await;
    assert_eq!(on_phone.pinned_apps, ["telegram", "notes"]);

    // Installed again (an update), the unpinned one stays unpinned.
    install(&mut mac, addr, "charts", true, &["telegram", "notes"]).await;

    // Only installed apps can be pinned.
    let refused = mac.request(Request::PinApp(v1::PinApp { app_id: "nope".into(), pinned: true, position: 0 })).await.body;
    assert!(matches!(&refused, Body::Error(error) if error.code() == v1::ErrorCode::InvalidRequest), "{refused:?}");

    // Uninstalled, it leaves the dock; installed afresh, it's pinned again.
    let events = mac.ok(Request::UninstallApp(v1::UninstallApp { id: "notes".into(), keep_data: false })).await;
    assert_eq!(last_workspace(&events).expect("the workspace").pinned_apps, ["telegram"]);
    install(&mut mac, addr, "notes", true, &["telegram", "notes"]).await;

    // Kept in factory.db, not config.json.
    let db = rusqlite::Connection::open(home_of(addr).db_path()).expect("factory.db");
    let mut rows: Vec<(String, Option<i64>)> = db
        .prepare("SELECT app, position FROM app_pins ORDER BY app")
        .expect("app_pins")
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("rows")
        .map(|row| row.expect("a row"))
        .collect();
    rows.sort();
    assert_eq!(rows, [("charts".into(), None), ("notes".into(), Some(1)), ("telegram".into(), Some(0))]);
    let config = std::fs::read_to_string(home_of(addr).config_path()).expect("config.json");
    assert!(!config.contains("pinned"), "{config}");
}
