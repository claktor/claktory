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
    let reply = apply_request(&mut client, &mut view, Request::ShuffleAvatar(v1::ShuffleAvatar::default())).await;
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

#[tokio::test]
async fn the_person_picks_a_pair_from_the_palette_and_a_plain_shuffle_forgets_it() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };

    // The palette comes with the person: 12 named pairs, and which is shown.
    let first = person(&view);
    assert_eq!(first.avatar_choices.len(), 12);
    assert!(first.avatar_choices.iter().all(|pair| pair.from.starts_with('#') && pair.to.starts_with('#') && !pair.name.is_empty()));
    let shown = usize::try_from(first.avatar_choice).expect("an index");
    assert_eq!(first.avatar_choices[shown].from, first.avatar_from);

    // Picking a pair shows it and remembers the choice in config.json.
    let picked = (shown + 3) % 12;
    let request = v1::ShuffleAvatar { choice: Some(i32::try_from(picked).unwrap()) };
    let reply = apply_request(&mut client, &mut view, Request::ShuffleAvatar(request)).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    view.follow(&mut client, |view| person(view).avatar_choice == i32::try_from(picked).unwrap()).await;
    let chosen = person(&view);
    assert_eq!(chosen.avatar_from, chosen.avatar_choices[picked].from);
    assert_eq!(chosen.avatar_to, chosen.avatar_choices[picked].to);
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("json");
    assert_eq!(config["user"]["avatar"]["choice"], serde_json::json!(picked), "{config}");

    // A pair the palette doesn't have is refused.
    let reply = apply_request(&mut client, &mut view, Request::ShuffleAvatar(v1::ShuffleAvatar { choice: Some(12) })).await;
    let Body::Error(error) = reply else { panic!("{reply:?}") };
    assert_eq!(error.message, "choice must be 0 to 11");
    assert_eq!(person(&view).avatar_choice, i32::try_from(picked).unwrap(), "the pick stays");

    // The pick survives a restart.
    drop(client);
    let (restarted, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(restarted).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(person(&view).avatar_choice, i32::try_from(picked).unwrap());

    // A plain shuffle forgets the choice: other colours, seed-driven again.
    let reply = apply_request(&mut client, &mut view, Request::ShuffleAvatar(v1::ShuffleAvatar::default())).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    view.follow(&mut client, |view| person(view).avatar_choice != i32::try_from(picked).unwrap()).await;
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("json");
    assert!(config["user"]["avatar"].get("choice").is_none(), "a plain shuffle clears the choice: {config}");
    assert!(config["user"]["avatar"]["seed"].as_str().is_some_and(|seed| !seed.is_empty()), "{config}");
}
