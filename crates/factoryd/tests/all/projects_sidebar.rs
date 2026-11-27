//! The projects sidebar, the backend's part: a project
//! folds in the sidebar (14), kept for every device and after a
//! restart, New project's caption asks where its folder would be (16),
//! deleting a project takes its pipes (17), a drag can be undone (15), and a
//! move to another project says which pipes change hands, and leaves them
//! (17b).

use factory_protocol::v1::{self, ErrorCode, client_message, server_message::Body};
use factoryd::pipes;

use crate::common::{Client, View, apply_request, create_agent, create_project, start, temp_dir};

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

/// Item 17 (G8): deleting a project closes its open pipes and deletes all
/// its pipes, closed ones too, with their messages, and its pipe groups;
/// another project's stay.
#[tokio::test]
async fn deleting_a_project_closes_and_deletes_its_pipes() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    for name in ["Site", "Shop"] {
        apply_request(&mut client, &mut view, create_project(name, &dir.join(name.to_lowercase()))).await;
    }
    let open = pipes::open_for_test(&factory, "site", "Feedback · Ana", &[], "", false);
    let closed = pipes::open_for_test(&factory, "site", "Feedback · Bo", &[], "", false);
    let other = pipes::open_for_test(&factory, "shop", "Orders", &[], "", false);
    pipes::sent_for_test(&factory, &open, "hello", true);
    let close = v1::SetPipeState { pipe_id: closed.clone(), action: v1::PipeAction::Close.into() };
    apply_request(&mut client, &mut view, client_message::Body::SetPipeState(close)).await;
    let group = v1::CreatePipeGroup { project_id: "site".into(), name: "Friends".into(), pipe_id: open.clone() };
    apply_request(&mut client, &mut view, client_message::Body::CreatePipeGroup(group)).await;
    let shop_group = v1::CreatePipeGroup { project_id: "shop".into(), name: "Orders".into(), pipe_id: other.clone() };
    apply_request(&mut client, &mut view, client_message::Body::CreatePipeGroup(shop_group)).await;

    let delete = client_message::Body::DeleteProject(v1::DeleteProject { project_id: "site".into() });
    assert!(matches!(apply_request(&mut client, &mut view, delete).await, Body::Ack(_)));
    view.follow(&mut client, |view| !view.workspace.pipes.iter().any(|pipe| pipe.id == open || pipe.id == closed)).await;
    for gone in [&open, &closed] {
        assert!(pipes::messages_for_test(&factory, gone).is_empty(), "its messages here go too");
    }
    assert_eq!(view.workspace.pipes.iter().map(|pipe| pipe.id.as_str()).collect::<Vec<_>>(), [other.as_str()]);
    assert_eq!(view.workspace.pipes[0].state(), v1::PipeState::Open, "another project's pipe stays open");
    // A project made again with its id starts with no pipes or groups.
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    let groups: Vec<(&str, &str)> =
        view.workspace.pipe_groups.iter().map(|group| (group.project_id.as_str(), group.name.as_str())).collect();
    assert_eq!(groups, [("shop", "Orders")]);
}

fn arrange(agent: &str, parent: &str, project: Option<&str>, undoable: bool) -> client_message::Body {
    client_message::Body::ArrangeAgent(v1::ArrangeAgent {
        agent_id: agent.into(),
        parent_id: parent.into(),
        project_id: project.map(str::to_owned),
        undoable,
        ..Default::default()
    })
}

fn restore(token: &str) -> client_message::Body {
    client_message::Body::RestoreArrangement(v1::RestoreArrangement { token: token.into() })
}

/// Each agent's id, project, lead and folder, in the sidebar's order.
fn places(view: &View) -> Vec<(String, String, String, String)> {
    view.workspace
        .agents
        .iter()
        .map(|agent| (agent.id.clone(), agent.project_id.clone(), agent.lead_id.clone(), agent.folder_id.clone()))
        .collect()
}

fn refused_with(body: &Body) -> String {
    match body {
        Body::Error(error) => error.message.clone(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Site with Ana, and Bo reporting to her; Shop with Dee.
async fn team() -> (std::path::PathBuf, std::sync::Arc<factoryd::Factory>, Client, View) {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    for name in ["Site", "Shop"] {
        apply_request(&mut client, &mut view, create_project(name, &dir.join(name.to_lowercase()))).await;
    }
    for (project, name) in [("site", "Ana"), ("site", "Bo"), ("site", "Cy"), ("shop", "Dee")] {
        apply_request(&mut client, &mut view, create_agent(project, name, "")).await;
    }
    assert!(matches!(apply_request(&mut client, &mut view, arrange("bo", "ana", None, false)).await, Body::Ack(_)));
    (dir, factory, client, view)
}

/// Item 15: an undoable drag answers what it moved, and Undo puts exactly
/// that back, once; a drag without it is answered as before.
#[tokio::test]
async fn undo_puts_a_drag_back_once() {
    let (_dir, _factory, mut client, mut view) = team().await;
    let before = places(&view);

    let arranged = match apply_request(&mut client, &mut view, arrange("ana", "dee", None, true)).await {
        Body::Arranged(arranged) => arranged,
        other => panic!("expected Arranged, got {other:?}"),
    };
    let moved: Vec<(&str, &str, &str)> =
        arranged.moved.iter().map(|moved| (moved.agent_id.as_str(), moved.project_id.as_str(), moved.lead_id.as_str())).collect();
    assert_eq!(moved, [("ana", "site", ""), ("bo", "site", "ana")], "where each was");
    assert_eq!(arranged.token.len(), 32);
    assert_eq!(view.agent("bo").project_id, "shop", "her report went with her");

    assert!(matches!(apply_request(&mut client, &mut view, restore(&arranged.token)).await, Body::Ack(_)));
    view.follow(&mut client, |view| view.agent("ana").project_id == "site").await;
    assert_eq!(places(&view), before, "project, lead, folder and order");
    // Spent.
    let again = apply_request(&mut client, &mut view, restore(&arranged.token)).await;
    assert_eq!(refused_with(&again), "This can't be undone any more.");
    assert_eq!(refused_with(&apply_request(&mut client, &mut view, restore("nonsense")).await), "This can't be undone any more.");
}

/// Nothing goes back once the list changed under it; a newer drag replaces
/// the older one's Undo.
#[tokio::test]
async fn undo_is_refused_once_the_list_changed() {
    let (_dir, _factory, mut client, mut view) = team().await;
    let token = |body: Body| match body {
        Body::Arranged(arranged) => arranged.token,
        other => panic!("expected Arranged, got {other:?}"),
    };
    let first = token(apply_request(&mut client, &mut view, arrange("cy", "ana", None, true)).await);
    let second = token(apply_request(&mut client, &mut view, arrange("ana", "dee", None, true)).await);
    assert_eq!(refused_with(&apply_request(&mut client, &mut view, restore(&first)).await), "This can't be undone any more.");
    // Cy, which went with Ana, moved again since.
    assert!(matches!(apply_request(&mut client, &mut view, arrange("cy", "", Some("site"), false)).await, Body::Ack(_)));
    let now = places(&view);
    let refused = apply_request(&mut client, &mut view, restore(&second)).await;
    assert_eq!(refused_with(&refused), "The list changed since, so this can't be undone.");
    assert_eq!(places(&view), now, "nothing changed");
}

/// Item 17b: a move's confirm names the pipes a moving agent stops
/// answering and who'd answer each after; the move takes it out of its old
/// project's pipe, but for an agent limited to it, the person's containment.
#[tokio::test]
async fn a_move_out_of_a_project_leaves_its_pipes() {
    let (_dir, factory, mut client, mut view) = team().await;
    // Ana leads it; Cy is limited to it.
    let pipe = pipes::open_for_test(&factory, "site", "Feedback · Ana", &["ana", "cy"], "ana", false);
    let limited =
        v1::UpdatePipe { pipe_id: pipe.clone(), quarantined: Some(v1::PipeMembers { agent_ids: vec!["cy".into()] }), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdatePipe(limited)).await;
    let preview = |agent: &str, parent: &str, project: Option<&str>| {
        client_message::Body::PreviewArrange(v1::PreviewArrange {
            agent_id: agent.into(),
            parent_id: parent.into(),
            project_id: project.map(str::to_owned),
            ..Default::default()
        })
    };
    let handovers = |body: Body| match body {
        Body::ArrangePreview(preview) => {
            preview.handovers.into_iter().map(|handover| (handover.pipe_id, handover.agent_id, handover.next_lead_id)).collect::<Vec<_>>()
        }
        other => panic!("expected a preview, got {other:?}"),
    };
    let before = places(&view);
    let said = handovers(apply_request(&mut client, &mut view, preview("ana", "dee", None)).await);
    assert_eq!(said, [(pipe.clone(), "ana".to_owned(), "cy".to_owned())]);
    assert_eq!(places(&view), before, "a preview moves nothing");
    // Within the project, or an agent that doesn't lead it: no handover.
    assert!(handovers(apply_request(&mut client, &mut view, preview("ana", "", Some("site"))).await).is_empty());
    assert!(handovers(apply_request(&mut client, &mut view, preview("bo", "dee", None)).await).is_empty());

    // Ana (and Bo, her report) move; Cy, limited, goes too but stays in it.
    apply_request(&mut client, &mut view, arrange("cy", "ana", None, false)).await;
    apply_request(&mut client, &mut view, arrange("ana", "dee", None, false)).await;
    let shown = |view: &View| view.workspace.pipes.iter().find(|shown| shown.id == pipe).cloned().expect("the pipe");
    view.follow(&mut client, |view| !shown(view).member_ids.contains(&"ana".to_owned())).await;
    assert_eq!(shown(&view).member_ids, ["cy"]);
    assert_eq!(shown(&view).quarantined_ids, ["cy"]);
}
