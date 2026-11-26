//! Notifications that wait (#364 O3): a Needs-you item that comes during
//! quiet hours notifies once they end, all of them in one notification, if
//! it still waits. And only apps that show notifications count as reaching
//! the person (#366 A2): Android's can't yet.

use std::time::Duration;

use crate::common::{Client, View, apply_request, create_agent, create_project, local_token, start, temp_dir};
use crate::notify::call;
use factory_protocol::v1::{self, NotifyKind, client_message, server_message::Body};
use factory_protocol::{PROTOCOL_VERSION, v1::ClientMessage};

fn quiet(on: bool, start: u32, end: u32) -> client_message::Body {
    let change = v1::NotificationSettingsChange {
        quiet_hours: Some(on),
        quiet_start: Some(start),
        quiet_end: Some(end),
        ..Default::default()
    };
    client_message::Body::UpdateSettings(v1::UpdateSettings { notification_change: Some(change), ..Default::default() })
}

fn ask(title: &str) -> serde_json::Value {
    serde_json::json!({"title": title, "body": "Only you can say."})
}

/// Connects as an app on `platform` (Hello.platform).
async fn connect_on(addr: std::net::SocketAddr, platform: &str) -> (Client, View) {
    let mut client = Client::open(addr).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "test".into(),
        token: local_token(addr),
        device_kind: v1::DeviceKind::Phone.into(),
        platform: platform.into(),
        ..Default::default()
    };
    client.send(ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) }).await;
    let (workspace, _) = client.welcomed().await;
    (client, View { workspace, ..View::default() })
}

#[tokio::test]
async fn needs_you_during_quiet_hours_notifies_once_they_end() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, mut view) = connect_on(addr, "macos").await;
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Helper", "")).await;
    let now = jiff::ZonedDateTime::now().time();
    let minute = u32::try_from(now.hour()).expect("hour") * 60 + u32::try_from(now.minute()).expect("minute");
    // Quiet from a minute ago for the next twelve hours.
    let (start, end) = ((minute + 1439) % 1440, (minute + 720) % 1440);
    assert!(matches!(apply_request(&mut client, &mut view, quiet(true, start, end)).await, Body::Ack(_)));

    let mut notes = Vec::new();
    call(&mut client, &mut view, &mut notes, "builder", "ask_person", ask("Deploy tonight?")).await;
    call(&mut client, &mut view, &mut notes, "helper", "ask_person", ask("Which logo?")).await;
    assert!(notes.is_empty(), "quiet: {notes:?}");

    // Quiet hours end (here, turned off): one notification for both.
    let reply = client.request(quiet(false, start, end)).await;
    notes.extend(reply.events.iter().filter_map(|event| match &event.body {
        Some(Body::Notify(notify)) => Some(notify.clone()),
        _ => None,
    }));
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        while notes.is_empty() {
            if let Some(Body::Notify(notify)) = client.receive().await.body {
                notes.push(notify);
            }
        }
    })
    .await;
    assert!(waited.is_ok(), "no notification when quiet hours ended");
    let [morning] = notes.as_slice() else { panic!("{notes:?}") };
    assert_eq!(morning.kind(), NotifyKind::NeedsYou);
    assert_eq!((morning.title.as_str(), morning.body.as_str()), ("2 things need you", "They came during your quiet hours."));
    assert!(morning.agent_id.is_empty() && morning.pipe_id.is_empty() && morning.chat_id.is_empty() && morning.device_id.is_empty());
    assert_eq!(morning.thread, "quiet");

    // Told once: quiet again and over again, nothing more.
    notes.clear();
    apply_request(&mut client, &mut view, quiet(true, start, end)).await;
    apply_request(&mut client, &mut view, quiet(false, start, end)).await;
    let more = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(Body::Notify(notify)) = client.receive().await.body {
                return notify;
            }
        }
    })
    .await;
    assert!(more.is_err(), "{more:?}");
}

/// An alert during quiet hours isn't pushed, but waits in Needs you and
/// counts in the one notification when they end (Bruno 44235).
#[tokio::test]
async fn an_alert_during_quiet_hours_waits_in_needs_you() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, mut view) = connect_on(addr, "macos").await;
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let now = jiff::ZonedDateTime::now().time();
    let minute = u32::try_from(now.hour()).expect("hour") * 60 + u32::try_from(now.minute()).expect("minute");
    let (start, end) = ((minute + 1439) % 1440, (minute + 720) % 1440);
    assert!(matches!(apply_request(&mut client, &mut view, quiet(true, start, end)).await, Body::Ack(_)));

    let mut notes = Vec::new();
    let alert = serde_json::json!({"title": "The build is done", "text": "All green."});
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert).await;
    assert_eq!(
        said,
        "Not sent as a notification: it's the person's quiet hours. It waits in their Needs you until they read it. When quiet hours end, they're told what came meanwhile."
    );
    assert!(notes.is_empty(), "quiet: {notes:?}");
    assert_eq!(view.workspace.alerts.iter().map(|alert| alert.title.as_str()).collect::<Vec<_>>(), ["The build is done"]);

    let reply = client.request(quiet(false, start, end)).await;
    notes.extend(reply.events.iter().filter_map(|event| match &event.body {
        Some(Body::Notify(notify)) => Some(notify.clone()),
        _ => None,
    }));
    let waited = tokio::time::timeout(Duration::from_secs(20), async {
        while notes.is_empty() {
            if let Some(Body::Notify(notify)) = client.receive().await.body {
                notes.push(notify);
            }
        }
    })
    .await;
    assert!(waited.is_ok(), "no notification when quiet hours ended");
    let [morning] = notes.as_slice() else { panic!("{notes:?}") };
    assert_eq!((morning.title.as_str(), morning.body.as_str()), ("1 thing needs you", "It came during your quiet hours."));
}

#[tokio::test]
async fn an_alert_with_only_an_android_app_connected_isnt_delivered() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, mut view) = connect_on(addr, "android").await;
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let mut notes = Vec::new();
    let alert = serde_json::json!({"title": "The build is done", "text": "All green."});
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert.clone()).await;
    assert!(said.starts_with("Not delivered as a notification:"), "{said}");

    // With the Mac's app connected too, it is.
    let (_mac, _) = connect_on(addr, "macos").await;
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert).await;
    assert_eq!(said, "Sent: the person gets it as a notification, and it waits in their Needs you until they read it.");
}
