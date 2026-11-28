//! A Claktory's own name and color: the same on every device, kept in
//! config.json once someone sets them, and until then this computer's name
//! and a color from its key, written nowhere.

use crate::common::{Client, View, last_workspace, start, temp_dir};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ClientMessage, client_message, server_message::Body};

const PALETTE: [&str; 8] = ["blue", "violet", "green", "pink", "teal", "amber", "red", "slate"];

fn identity(workspace: &v1::Workspace) -> (String, String) {
    let settings = workspace.settings.as_ref().expect("settings");
    (settings.name.clone(), settings.color.clone())
}

fn set(name: Option<&str>, color: Option<&str>) -> client_message::Body {
    client_message::Body::UpdateSettings(v1::UpdateSettings {
        name: name.map(str::to_owned),
        color: color.map(str::to_owned),
        ..Default::default()
    })
}

/// What Welcome says to a new connection.
async fn welcome(addr: std::net::SocketAddr) -> v1::Welcome {
    let mut client = Client::open(addr).await;
    let hello = v1::Hello {
        protocol_version: PROTOCOL_VERSION,
        client_version: "test".into(),
        token: crate::common::local_token(addr),
        ..Default::default()
    };
    client.send(ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) }).await;
    match client.receive().await.body {
        Some(Body::Welcome(welcome)) => welcome,
        other => panic!("expected welcome, got {other:?}"),
    }
}

#[tokio::test]
async fn a_claktory_has_one_name_and_color_for_every_device() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let config = || std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    let (mut client, workspace, _) = Client::connect(addr).await;

    // Until set: this computer's name and a color of the palette, the same
    // in Welcome, and nothing written.
    let (name, color) = identity(&workspace);
    assert_eq!(name, workspace.remote.as_ref().expect("remote").machine);
    assert!(PALETTE.contains(&color.as_str()), "{color}");
    let hello = welcome(addr).await;
    assert_eq!((hello.name, hello.color), (name.clone(), color.clone()));
    let saved: serde_json::Value = serde_json::from_str(&config()).expect("json");
    assert!(saved.get("name").is_none() && saved.get("color").is_none(), "only written once set");

    // Set from one device, every device sees it, and so does a new one.
    let other_color = PALETTE.iter().find(|other| **other != color).expect("another color");
    let events = client.ok(set(Some("  Studio  "), Some(other_color))).await;
    assert_eq!(identity(last_workspace(&events).expect("workspace")), ("Studio".to_owned(), other_color.to_string()));
    let saved: serde_json::Value = serde_json::from_str(&config()).expect("json");
    assert_eq!((saved["name"].as_str(), saved["color"].as_str()), (Some("Studio"), Some(*other_color)));
    let (_, fresh, _) = Client::connect(addr).await;
    assert_eq!(identity(&fresh), ("Studio".to_owned(), other_color.to_string()));
    let hello = welcome(addr).await;
    assert_eq!((hello.name.as_str(), hello.color.as_str()), ("Studio", *other_color));

    // A color off the palette, or a name with a line break, is refused.
    for refused in [set(None, Some("orange")), set(Some("Two\nlines"), None)] {
        let reply = client.request(refused).await;
        assert!(matches!(&reply.body, Body::Error(_)), "{:?}", reply.body);
    }

    // Empty goes back to the defaults, and the file forgets them.
    let events = client.ok(set(Some(""), Some(""))).await;
    assert_eq!(identity(last_workspace(&events).expect("workspace")), (name.clone(), color.clone()));
    let saved: serde_json::Value = serde_json::from_str(&config()).expect("json");
    assert!(saved.get("name").is_none() && saved.get("color").is_none());

    // Rivet changes them by editing config.json.
    let mut edited: serde_json::Value = serde_json::from_str(&config()).expect("json");
    edited["name"] = "Lee's Mac".into();
    edited["color"] = "amber".into();
    std::fs::write(dir.join("home/config.json"), serde_json::to_string_pretty(&edited).expect("json")).expect("write");
    let mut view = View::default();
    view.follow(&mut client, |view| view.workspace.settings.as_ref().is_some_and(|settings| settings.name == "Lee's Mac")).await;
    assert_eq!(identity(&view.workspace), ("Lee's Mac".to_owned(), "amber".to_owned()));
}

/// A name that could reorder or hide what's around it where
/// it shows (bidi controls, zero widths, the BOM) is refused from a device,
/// and taken out of one written in config.json, which still reads; an
/// emoji's own variation selector stays.
#[tokio::test]
async fn names_that_hide_or_reorder_text_are_refused_or_cleaned() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let config = || std::fs::read_to_string(dir.join("home/config.json")).expect("config.json");
    let (mut client, _, _) = Client::connect(addr).await;

    let person = |name: &str| client_message::Body::UpdateSettings(v1::UpdateSettings { user_name: Some(name.into()), ..Default::default() });
    let agent = |name: Option<&str>, role: Option<&str>| {
        client_message::Body::UpdateAgent(v1::UpdateAgent {
            agent_id: "rivet".into(),
            name: name.map(str::to_owned),
            role: role.map(str::to_owned),
            ..Default::default()
        })
    };
    let project = |name: &str| client_message::Body::CreateProject(v1::CreateProject { name: name.into(), path: String::new() });
    for refused in [
        set(Some("Studio\u{202E}gpj.exe"), None),
        set(Some("Stu\u{200B}dio"), None),
        set(Some("\u{2066}Studio\u{2069}"), None),
        person("Ma\u{FEFF}tt"),
        person("Matt\u{061C}"),
        agent(Some("Ri\u{200D}vet"), None),
        agent(None, Some("Admin\u{200E}")),
        agent(Some("Rivet\u{2060}"), None),
        project("Site\u{202A}"),
    ] {
        let reply = client.request(refused.clone()).await;
        let Body::Error(error) = &reply.body else { panic!("{refused:?} wasn't refused: {:?}", reply.body) };
        assert!(error.message.contains("can't have line breaks or hidden characters"), "{}", error.message);
        assert!(!error.message.chars().any(factory_agent::text::hidden), "the refusal shows the name without them: {:?}", error.message);
    }
    let saved: serde_json::Value = serde_json::from_str(&config()).expect("json");
    assert!(saved.get("name").is_none(), "nothing was written");

    // An emoji keeps its own variation selector.
    let events = client.ok(set(Some("Studio \u{2764}\u{FE0F}"), None)).await;
    assert_eq!(identity(last_workspace(&events).expect("workspace")).0, "Studio \u{2764}\u{FE0F}");

    // Written in the file, they're taken out, and the file still reads.
    let mut edited: serde_json::Value = serde_json::from_str(&config()).expect("json");
    edited["name"] = "Lee\u{202E}'s\u{200B} Mac".into();
    let at = edited["agents"].as_array().expect("agents").iter().position(|agent| agent["id"] == "rivet").expect("rivet");
    edited["agents"][at]["name"] = "Ri\u{2067}vet\u{FEFF}".into();
    edited["agents"][at]["role"] = "\u{200F}Admin\nhelper".into();
    std::fs::write(dir.join("home/config.json"), serde_json::to_string_pretty(&edited).expect("json")).expect("write");
    let mut view = View::default();
    view.follow(&mut client, |view| view.workspace.settings.as_ref().is_some_and(|settings| settings.name == "Lee's Mac")).await;
    let rivet = view.workspace.agents.iter().find(|agent| agent.id == "rivet").expect("rivet");
    assert_eq!((rivet.name.as_str(), rivet.role.as_str()), ("Rivet", "Admin helper"));
    assert!(view.workspace.config_error.is_empty(), "{}", view.workspace.config_error);
}
