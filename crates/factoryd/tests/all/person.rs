//! The person this Claktory belongs to (Workspace.person): their initials
//! follow their name, their avatar's colours come from the Claktory's key
//! until they shuffle them, and stay the same across a restart.

use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};

use crate::common::{Client, View, apply_request, start, temp_dir};

fn person(view: &View) -> v1::Person {
    view.workspace.person.clone().expect("the person is always set")
}

#[tokio::test]
async fn the_persons_avatar_follows_their_name_and_shuffles_for_every_device() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };

    // No name yet: no initials, colours from the key all the same.
    let first = person(&view);
    assert_eq!((first.name.as_str(), first.initials.as_str()), ("", ""));
    assert!(first.avatar_from.starts_with('#') && first.avatar_to.len() == 7, "{first:?}");
    assert!(first.identity_id.is_empty() && first.email.is_empty());

    let named = v1::UpdateSettings { user_name: Some("Matt Smith".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateSettings(named)).await;
    view.follow(&mut client, |view| person(view).initials == "MS").await;
    assert_eq!(person(&view).avatar_from, first.avatar_from, "a new name keeps the colours");
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("json");
    assert!(config["user"].get("avatar").is_none(), "nothing written until shuffled: {}", config["user"]);

    // Shuffled: other colours, for every device, kept in config.json.
    let (mut other, other_workspace, _) = Client::connect(addr).await;
    let mut other_view = View { workspace: other_workspace, ..View::default() };
    let reply = apply_request(&mut client, &mut view, Request::ShuffleAvatar(v1::ShuffleAvatar {})).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    view.follow(&mut client, |view| person(view).avatar_from != first.avatar_from || person(view).avatar_to != first.avatar_to).await;
    let shuffled = person(&view);
    other_view.follow(&mut other, |view| person(view) == shuffled).await;
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("json");
    assert!(config["user"]["avatar"]["seed"].as_str().is_some_and(|seed| !seed.is_empty()), "{config}");

    // The same after a restart.
    drop(client);
    let (restarted, _factory) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(restarted).await;
    assert_eq!(workspace.person.expect("person"), shuffled);
}
