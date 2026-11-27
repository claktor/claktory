//! Projects are dragged into the order the person wants in
//! the sidebar, kept as config.json's list of projects, the same on every
//! device and after a restart; archived ones keep their place.

use crate::common::{Client, Scripted, View, apply_request, create_project, start_with, temp_dir};
use factory_protocol::v1::{self, ErrorCode, client_message, server_message::Body};

fn drag(project: &str, before: &str) -> client_message::Body {
    client_message::Body::UpdateProject(v1::UpdateProject {
        project_id: project.into(),
        before_id: Some(before.into()),
        ..Default::default()
    })
}

fn order(view: &View) -> Vec<String> {
    view.workspace.projects.iter().map(|project| project.id.clone()).collect()
}

#[tokio::test]
async fn projects_are_dragged_into_the_order_the_person_wants() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    for name in ["Site", "Lab", "Ops"] {
        apply_request(&mut client, &mut view, create_project(name, &dir.join(name.to_lowercase()))).await;
    }
    assert_eq!(order(&view), ["site", "lab", "ops"], "new ones go last");

    // Ops to the top, as the most used; then Site last.
    assert!(matches!(apply_request(&mut client, &mut view, drag("ops", "site")).await, Body::Ack(_)));
    assert_eq!(order(&view), ["ops", "site", "lab"]);
    apply_request(&mut client, &mut view, drag("site", "")).await;
    assert_eq!(order(&view), ["ops", "lab", "site"]);
    // Before itself changes nothing; unset leaves the order; a name and a
    // place in one request both hold.
    apply_request(&mut client, &mut view, drag("lab", "lab")).await;
    let rename = v1::UpdateProject { project_id: "lab".into(), name: Some("Research".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateProject(rename)).await;
    assert_eq!(order(&view), ["ops", "lab", "site"]);
    let both = v1::UpdateProject { project_id: "site".into(), name: Some("Web".into()), archived: None, before_id: Some("ops".into()) };
    apply_request(&mut client, &mut view, client_message::Body::UpdateProject(both)).await;
    assert_eq!(order(&view), ["site", "ops", "lab"]);
    assert_eq!((view.workspace.projects[0].name.as_str(), view.workspace.projects[2].name.as_str()), ("Web", "Research"));

    // Archived, it keeps its place, and moves like any other.
    let archive = v1::UpdateProject { project_id: "ops".into(), archived: Some(true), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateProject(archive)).await;
    assert_eq!(order(&view), ["site", "ops", "lab"]);
    apply_request(&mut client, &mut view, drag("ops", "")).await;
    assert_eq!(order(&view), ["site", "lab", "ops"]);

    // A drag from a list that changed meanwhile is refused, and nothing moves.
    match apply_request(&mut client, &mut view, drag("lab", "gone")).await {
        Body::Error(error) => assert_eq!(
            (error.code(), error.message.as_str()),
            (ErrorCode::InvalidRequest, "The list changed while you were dragging. Try again.")
        ),
        other => panic!("expected an error, got {other:?}"),
    }
    assert!(
        matches!(apply_request(&mut client, &mut view, drag("gone", "")).await, Body::Error(error) if error.code() == ErrorCode::NotFound)
    );
    assert_eq!(order(&view), ["site", "lab", "ops"]);

    // Kept in config.json's order, and so after a restart.
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("json");
    let written: Vec<&str> = config["projects"].as_array().expect("projects").iter().filter_map(|project| project["id"].as_str()).collect();
    assert_eq!(written, ["site", "lab", "ops"]);
    let (addr, _again) = start_with(&dir, Scripted::signed_in()).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert_eq!(order(&View { workspace, ..View::default() }), ["site", "lab", "ops"]);
}
