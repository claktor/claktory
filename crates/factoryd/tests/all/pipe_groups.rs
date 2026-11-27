//! Pipe groups: a project's pipes gathered under named rows that fold
//! away the same on every device, arranged by the person or by Rivet.
//! Arrangement only, so pipes work as before; deleting a group keeps its
//! pipes.

use std::sync::Arc;

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_project, start_with_models, temp_dir};
use crate::pipes::call;
use factory_protocol::v1::{self, ErrorCode, client_message::Body as Request, server_message::Body};
use factoryd::pipes;

fn group_named<'a>(view: &'a View, name: &str) -> &'a v1::PipeGroup {
    view.workspace.pipe_groups.iter().find(|group| group.name == name).expect("the group")
}

fn group_of(view: &View, pipe: &str) -> String {
    view.workspace.pipes.iter().find(|known| known.id == pipe).expect("the pipe").group_id.clone()
}

async fn refused(client: &mut Client, view: &mut View, request: Request) -> (ErrorCode, String) {
    match apply_request(client, view, request).await {
        Body::Error(error) => (error.code(), error.message),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn pipes_gather_into_groups_that_fold_away_and_deleting_one_keeps_its_pipes() {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = serde_json::json!({
        "agents": [{"id": "rivet", "name": "Rivet", "model": MODEL, "effort": "off", "extensions": {"agents": {"admin": true}}}],
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start_with_models(&dir, Arc::new(Scripted::signed_in())).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_project("Shop", &dir.join("shop"))).await;
    let ana = pipes::open_for_test(&factory, "site", "Feedback · Ana", &[], "", false);
    let bo = pipes::open_for_test(&factory, "site", "Feedback · Bo", &[], "", false);
    let other = pipes::open_for_test(&factory, "shop", "Orders", &[], "", false);
    assert!(view.workspace.pipe_groups.is_empty());

    // New group… from Ana's pipe: made, with her pipe in it, open.
    let create = |name: &str, pipe: &str| {
        Request::CreatePipeGroup(v1::CreatePipeGroup { project_id: "site".into(), name: name.into(), pipe_id: pipe.into() })
    };
    assert!(matches!(apply_request(&mut client, &mut view, create("  Friend\nfeedback ", &ana)).await, Body::Ack(_)));
    let made = group_named(&view, "Friend feedback").clone();
    assert_eq!((made.project_id.as_str(), made.collapsed, made.position), ("site", false, 0));
    assert!(made.created_ms > 0);
    assert_eq!((group_of(&view, &ana), group_of(&view, &bo)), (made.id.clone(), String::new()));

    // Names: one line, needed, and one per project.
    assert_eq!(
        refused(&mut client, &mut view, create(" \n ", "")).await,
        (ErrorCode::InvalidRequest, "Give the group a name.".to_owned())
    );
    assert_eq!(
        refused(&mut client, &mut view, create("friend FEEDBACK", "")).await,
        (ErrorCode::InvalidRequest, "This project already has a group named friend FEEDBACK.".to_owned())
    );
    // A pipe only in its own project's group.
    let into_other = Request::SetPipeGroup(v1::SetPipeGroup { pipe_id: other.clone(), group_id: made.id.clone() });
    assert_eq!(
        refused(&mut client, &mut view, into_other).await,
        (ErrorCode::InvalidRequest, "A pipe can only be in a group of its own project.".to_owned())
    );

    // Bo's moves in; the group folds away, the same on every device.
    let bo_in = Request::SetPipeGroup(v1::SetPipeGroup { pipe_id: bo.clone(), group_id: made.id.clone() });
    assert!(matches!(apply_request(&mut client, &mut view, bo_in).await, Body::Ack(_)));
    assert_eq!(group_of(&view, &bo), made.id);
    let fold = Request::SetPipeGroupCollapsed(v1::SetPipeGroupCollapsed { group_id: made.id.clone(), collapsed: true });
    assert!(matches!(apply_request(&mut client, &mut view, fold).await, Body::Ack(_)));
    assert!(group_named(&view, "Friend feedback").collapsed);
    let (_other, workspace, _) = Client::connect(addr).await;
    assert!(workspace.pipe_groups.iter().any(|group| group.id == made.id && group.collapsed), "another device sees it folded");

    // A second group comes after it; renamed, it keeps its place.
    assert!(matches!(apply_request(&mut client, &mut view, create("Partners", "")).await, Body::Ack(_)));
    let partners = group_named(&view, "Partners").clone();
    assert_eq!(partners.position, 1);
    let rename = Request::RenamePipeGroup(v1::RenamePipeGroup { group_id: partners.id.clone(), name: "Agencies".into() });
    assert!(matches!(apply_request(&mut client, &mut view, rename).await, Body::Ack(_)));
    assert_eq!(group_named(&view, "Agencies").position, 1);

    // Rivet arranges them too, in Claktory's words.
    let said = call(&mut client, &mut view, "rivet", "pipes_list", r#"{"project": "Site"}"#).await;
    assert!(said.contains(&format!("in Site, in the group Friend feedback (id {})", made.id)), "{said}");
    assert!(said.contains(&format!("- Groups in Site: Friend feedback (id {}, 2 pipes), Agencies (id {}, 0 pipes)", made.id, partners.id)), "{said}");
    // Named here, a pipe is known by its name (one the other side chose
    // never is, as pipes_send's lookup).
    let said = call(&mut client, &mut view, "rivet", "pipes_group", r#"{"action": "move", "pipe": "Feedback · Bo", "group": "Agencies"}"#).await;
    assert_eq!(said, "Moved \"Feedback · Bo\" to the group Agencies.");
    view.follow(&mut client, |view| group_of(view, &bo) == partners.id).await;
    let args = format!(r#"{{"action": "create", "name": "Shop friends", "pipe": "{other}"}}"#);
    let said = call(&mut client, &mut view, "rivet", "pipes_group", &args).await;
    assert_eq!(said, "Made the group Shop friends in Shop, and moved \"Orders\" to it.");
    let unknown = call(&mut client, &mut view, "rivet", "pipes_group", r#"{"action": "move", "pipe": "Invoices", "group": ""}"#).await;
    assert!(unknown.contains("There's no pipe \"Invoices\""), "{unknown}");

    // Deleting a group keeps its pipes, outside any group.
    let said = call(&mut client, &mut view, "rivet", "pipes_group", r#"{"action": "delete", "group": "Friend feedback"}"#).await;
    assert_eq!(said, "Deleted the group Friend feedback; its pipe stays, outside any group.");
    view.follow(&mut client, |view| !view.workspace.pipe_groups.iter().any(|group| group.id == made.id)).await;
    assert_eq!(group_of(&view, &ana), "");
    assert!(view.workspace.pipes.iter().any(|pipe| pipe.id == ana), "the pipe stays");
    let delete = Request::DeletePipeGroup(v1::DeletePipeGroup { group_id: made.id.clone() });
    assert_eq!(refused(&mut client, &mut view, delete).await, (ErrorCode::NotFound, "That group was deleted.".to_owned()));

    // A deleted group's id is never given to another, so a device still
    // showing the deleted one can't rename, delete or fill the new one.
    assert!(matches!(apply_request(&mut client, &mut view, create("Newest", "")).await, Body::Ack(_)));
    let newest = group_named(&view, "Newest").clone();
    let delete = Request::DeletePipeGroup(v1::DeletePipeGroup { group_id: newest.id.clone() });
    assert!(matches!(apply_request(&mut client, &mut view, delete.clone()).await, Body::Ack(_)));
    assert!(matches!(apply_request(&mut client, &mut view, create("Newer", "")).await, Body::Ack(_)));
    let newer = group_named(&view, "Newer").clone();
    assert!(![made.id.as_str(), partners.id.as_str(), newest.id.as_str()].contains(&newer.id.as_str()), "{} reused", newer.id);
    let gone = (ErrorCode::NotFound, "That group was deleted.".to_owned());
    let stale = Request::RenamePipeGroup(v1::RenamePipeGroup { group_id: newest.id.clone(), name: "Renamed by a stale device".into() });
    assert_eq!(refused(&mut client, &mut view, stale).await, gone);
    let stale = Request::SetPipeGroup(v1::SetPipeGroup { pipe_id: ana.clone(), group_id: newest.id.clone() });
    assert_eq!(refused(&mut client, &mut view, stale).await, gone);
    assert_eq!(refused(&mut client, &mut view, delete).await, gone);
    assert_eq!(group_named(&view, "Newer").clone(), newer);

    // Out of its group, from the app.
    let out = Request::SetPipeGroup(v1::SetPipeGroup { pipe_id: bo.clone(), group_id: String::new() });
    assert!(matches!(apply_request(&mut client, &mut view, out).await, Body::Ack(_)));
    assert_eq!(group_of(&view, &bo), "");
}
