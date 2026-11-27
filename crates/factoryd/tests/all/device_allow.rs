//! Allowing a waiting device takes the four digits it shows, checked by the
//! backend, and each device that starts waiting notifies in Claktory's own
//! words, at most 3 an hour.

use std::time::Duration;

use crate::common::{Client, View, apply_request, start, temp_dir};
use crate::remote::{number, turned_away, waiting_device};
use factory_protocol::v1::{self, ErrorCode, client_message, server_message::Body};

/// Follows the Workspace until `done`, keeping the notifications on the way.
async fn follow(client: &mut Client, view: &mut View, notes: &mut Vec<v1::Notify>, mut done: impl FnMut(&View) -> bool) {
    let waited = tokio::time::timeout(Duration::from_secs(30), async {
        while !done(view) {
            let message = client.receive().await;
            let body = message.body.as_ref().expect("body");
            if let Body::Notify(notify) = body {
                notes.push(notify.clone());
            }
            view.apply(body);
        }
    })
    .await;
    assert!(waited.is_ok(), "still waiting: {:?}", view.workspace.remote);
}

fn pending(view: &View) -> Vec<v1::PendingDevice> {
    view.workspace.remote.as_ref().map(|remote| remote.pending_devices.clone()).unwrap_or_default()
}

fn allow(id: &str, number: &str) -> client_message::Body {
    client_message::Body::ApproveDevice(v1::ApproveDevice { device_id: id.into(), number: number.into(), ..Default::default() })
}

/// A number that isn't `right`.
fn wrong(right: &str) -> String {
    if right == "1234" { "4321".into() } else { "1234".into() }
}

async fn refused(client: &mut Client, view: &mut View, request: client_message::Body) -> String {
    match apply_request(client, view, request).await {
        Body::Error(error) if error.code() == ErrorCode::InvalidRequest => error.message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_device_is_allowed_only_with_its_number_and_notifies_in_claktorys_words() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::SetRemoteAccess(relays)).await;
    let machine = view.workspace.settings.clone().expect("settings").name;
    let mut notes = Vec::new();

    // A device waits: the notification is Claktory's sentence, never the
    // name it was given, and opens its Needs you item.
    let (code, id, device, _link, mut socket) = waiting_device(&mut client, &mut view, "Evil Name 7731").await;
    follow(&mut client, &mut view, &mut notes, |view| pending(view).len() == 1).await;
    // (Notifications go out just before the Workspace that shows why.)
    let devices: Vec<&v1::Notify> = notes.iter().filter(|note| !note.device_id.is_empty()).collect();
    let [note] = devices[..] else { panic!("{notes:?}") };
    assert_eq!(note.kind(), v1::NotifyKind::NeedsYou);
    assert_eq!((note.title.as_str(), note.device_id.as_str()), (machine.as_str(), id.as_str()));
    assert_eq!(note.body, format!("A device is waiting to be allowed on {machine}."));
    assert!(!note.title.contains("7731") && !note.body.contains("7731"));
    assert!(note.agent_id.is_empty() && note.pipe_id.is_empty() && note.chat_id.is_empty());

    // No number asks for it, and counts for nothing; wrong ones are refused,
    // and the third turns the device away.
    let right = number(&code, &device.id().to_string());
    let asked = refused(&mut client, &mut view, allow(&id, "")).await;
    assert_eq!(asked, "Type the 4 digits Evil Name 7731 shows to allow it.");
    let first = refused(&mut client, &mut view, allow(&id, &wrong(&right))).await;
    assert_eq!(first, "That's not the number Evil Name 7731 shows. 2 tries left.");
    let second = refused(&mut client, &mut view, allow(&id, &wrong(&right))).await;
    assert_eq!(second, "That's not the number Evil Name 7731 shows. 1 try left.");
    assert_eq!(pending(&view).len(), 1, "still waiting");
    let third = refused(&mut client, &mut view, allow(&id, &wrong(&right))).await;
    assert_eq!(third, "That's not the number Evil Name 7731 shows, a third time, so it was turned away. To add it, make a new code.");
    let why = turned_away(&mut socket, "three wrong numbers").await;
    assert!(why.contains("didn't allow this device"), "{why}");
    follow(&mut client, &mut view, &mut notes, |view| pending(view).is_empty()).await;
    let gone = apply_request(&mut client, &mut view, allow(&id, &right)).await;
    assert!(matches!(gone, Body::Error(ref error) if error.code() == ErrorCode::NotFound), "{gone:?}");

    // Two more notify; a fourth device within the hour doesn't, and shows
    // only in Needs you. The right number lets it in.
    let mut waiting = Vec::new();
    for name in ["Laptop", "Tablet", "Watch"] {
        let device = waiting_device(&mut client, &mut view, name).await;
        let count = pending(&view).len() + 1;
        follow(&mut client, &mut view, &mut notes, |view| pending(view).len() >= count).await;
        waiting.push(device);
    }
    follow(&mut client, &mut view, &mut notes, |view| pending(view).len() == 3).await;
    let sent: Vec<&str> = notes.iter().filter(|note| !note.device_id.is_empty()).map(|note| note.device_id.as_str()).collect();
    let ids: Vec<&str> = waiting.iter().map(|(_, id, ..)| id.as_str()).collect();
    assert_eq!(sent, [id.as_str(), ids[0], ids[1]], "the fourth in the hour notifies nothing");
    let (code, watch, device, _link, mut socket) = waiting.pop().expect("the watch");
    let right = number(&code, &device.id().to_string());
    let wrong_once = refused(&mut client, &mut view, allow(&watch, &wrong(&right))).await;
    assert!(wrong_once.ends_with("2 tries left."), "{wrong_once}");
    assert!(matches!(apply_request(&mut client, &mut view, allow(&watch, &right)).await, Body::Ack(_)));
    match crate::remote::next(&mut socket).await {
        Some(Body::Welcome(welcome)) => assert_eq!(welcome.device_id, watch),
        other => panic!("expected welcome, got {other:?}"),
    }

    // Restarted, the two still waiting notify nothing again: what was
    // noted is kept in factory.db. (The first Claktory runs on in this
    // process.)
    let again = factoryd::factory::Factory::open(crate::common::home(&dir), std::sync::Arc::new(crate::common::Scripted::signed_in()))
        .await
        .expect("open again");
    again.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut client, mut view) = crate::accounts::serve(&again, &dir).await;
    let mut notes = Vec::new();
    follow(&mut client, &mut view, &mut notes, |view| pending(view).len() == 2).await;
    // A few Workspaces more, as a start brings them.
    for name in ["Restarted", "Restarted again"] {
        let update = v1::UpdateSettings { name: Some(name.into()), ..Default::default() };
        apply_request(&mut client, &mut view, client_message::Body::UpdateSettings(update)).await;
        follow(&mut client, &mut view, &mut notes, |view| view.workspace.settings.as_ref().is_some_and(|settings| settings.name == name)).await;
    }
    assert!(notes.iter().all(|note| note.device_id.is_empty()), "{notes:?}");
}
