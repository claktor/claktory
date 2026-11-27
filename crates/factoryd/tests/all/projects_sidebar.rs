//! Area 5b, the backend's part of Lucy's ux/projects-sidebar.md §11: a
//! project folds in the sidebar (14), kept for every device and after a
//! restart, and New project's caption asks where its folder would be (16).

use factory_protocol::v1::{self, ErrorCode, client_message, server_message::Body};

use crate::common::{Client, View, apply_request, create_project, start, temp_dir};

fn fold(project: &str, collapsed: bool) -> client_message::Body {
    client_message::Body::SetProjectCollapsed(v1::SetProjectCollapsed { project_id: project.into(), collapsed })
}

fn collapsed(view: &View) -> Vec<(String, bool)> {
    view.workspace.projects.iter().map(|project| (project.id.clone(), project.collapsed)).collect()
}

async fn preview(client: &mut Client, name: &str) -> v1::ProjectFolderPreview {
    let body = client_message::Body::PreviewProjectFolder(v1::PreviewProjectFolder { name: name.into() });
    match client.request(body).await.body {
        Body::ProjectFolderPreview(preview) => preview,
        other => panic!("expected a preview, got {other:?}"),
    }
}

#[tokio::test]
async fn a_project_folds_for_every_device_and_after_a_restart() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    for name in ["Site", "Lab"] {
        apply_request(&mut client, &mut view, create_project(name, &dir.join(name.to_lowercase()))).await;
    }
    assert_eq!(collapsed(&view), [("site".to_owned(), false), ("lab".to_owned(), false)]);

    assert!(matches!(apply_request(&mut client, &mut view, fold("site", true)).await, Body::Ack(_)));
    assert_eq!(collapsed(&view), [("site".to_owned(), true), ("lab".to_owned(), false)]);
    // The whole Projects section is another thing, untouched.
    assert!(!view.workspace.projects_collapsed);
    // Another device sees it as it connects.
    let (_other, workspace, _) = Client::connect(addr).await;
    assert!(workspace.projects.iter().any(|project| project.id == "site" && project.collapsed));
    // Folding it again changes nothing; a project that isn't one is refused.
    assert!(matches!(apply_request(&mut client, &mut view, fold("site", true)).await, Body::Ack(_)));
    let refused = apply_request(&mut client, &mut view, fold("nowhere", true)).await;
    assert!(matches!(&refused, Body::Error(error) if error.code() == ErrorCode::NotFound), "{refused:?}");

    // Kept after a restart.
    drop(client);
    let (restarted, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(restarted).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(collapsed(&view), [("site".to_owned(), true), ("lab".to_owned(), false)]);
    apply_request(&mut client, &mut view, fold("lab", true)).await;
    apply_request(&mut client, &mut view, fold("site", false)).await;
    assert_eq!(collapsed(&view), [("site".to_owned(), false), ("lab".to_owned(), true)]);

    // A deleted project's fold goes with it: one made later with its id
    // starts unfolded.
    let delete = client_message::Body::DeleteProject(v1::DeleteProject { project_id: "lab".into() });
    assert!(matches!(apply_request(&mut client, &mut view, delete).await, Body::Ack(_)));
    apply_request(&mut client, &mut view, create_project("Lab", &dir.join("lab"))).await;
    assert_eq!(collapsed(&view), [("site".to_owned(), false), ("lab".to_owned(), false)]);
    drop(factory);
}

#[tokio::test]
async fn new_projects_caption_says_where_its_folder_would_be() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let workspace_folder = factory.home().workspace();

    let said = preview(&mut client, "Bakery Website").await;
    assert_eq!(said.name, "Bakery Website", "the name it answers, to tell answers apart");
    assert_eq!(said.path, workspace_folder.join("bakery-website").display().to_string());
    // As CreateProject would: with -2 once the id is taken.
    apply_request(&mut client, &mut view, create_project("Bakery website", std::path::Path::new(""))).await;
    let made = view.workspace.projects.iter().find(|project| project.name == "Bakery website").expect("made");
    assert_eq!(made.path, said.path, "where the caption said");
    assert_eq!(preview(&mut client, "Bakery  Website!").await.path, workspace_folder.join("bakery-website-2").display().to_string());
    // A name with nothing to make an id of takes the fallback; an empty one
    // gets no folder, as CreateProject refuses it.
    assert_eq!(preview(&mut client, "★★").await.path, workspace_folder.join("project").display().to_string());
    assert_eq!(preview(&mut client, "   ").await.path, "");
}
