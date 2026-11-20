//! Focusing a project shows only it, with its folder in the explorer.

use std::path::Path;

use crate::common::{Client, View, apply_request, create_agent, create_project, start, temp_dir};
use factory_protocol::v1::{self, ErrorCode, client_message, server_message::Body};

fn focus(project_id: &str) -> client_message::Body {
    client_message::Body::FocusProject(v1::FocusProject { project_id: project_id.into() })
}

fn expand(path: &str, expanded: bool) -> client_message::Body {
    client_message::Body::ExpandFolder(v1::ExpandFolder {
        project_id: "site".into(),
        path: path.into(),
        expanded,
        folder_id: String::new(),
        agent_id: String::new(),
    })
}

fn show_all(show_all: bool) -> client_message::Body {
    client_message::Body::ShowAllFiles(v1::ShowAllFiles { project_id: "site".into(), show_all })
}

#[tokio::test]
async fn a_focused_project_shows_its_folder_live_without_what_factoryignore_hides() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", Path::new(""))).await;
    let folder = dir.join("home/workspace/site");
    assert!(folder.join(".factoryignore").is_file(), "new projects say what's hidden");
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    for file in ["src/main.rs", "src/ui/app.rs", "Docs/intro.md", "README.md", "node_modules/left-pad/index.js"] {
        std::fs::create_dir_all(folder.join(file).parent().expect("parent")).expect("mkdir");
        std::fs::write(folder.join(file), "").expect("write");
    }

    // Focusing picks one of the project's agents and sends the tree before the ack.
    apply_request(&mut client, &mut view, focus("site")).await;
    assert_eq!(view.workspace.focused_project_id, "site");
    assert_eq!(view.workspace.selected_agent_id, "writer");
    assert_eq!(view.tree(), ["Docs/", "src/", ".factoryignore", "README.md"], "folders first; node_modules hidden");

    apply_request(&mut client, &mut view, expand("src", true)).await;
    apply_request(&mut client, &mut view, expand("src/ui", true)).await;
    assert_eq!(view.tree(), ["Docs/", "src/", "  ui/", "    app.rs", "  main.rs", ".factoryignore", "README.md"]);

    // What agents write shows up by itself.
    std::fs::write(folder.join("src/lib.rs"), "").expect("write");
    view.follow(&mut client, |view| view.tree().contains(&"  lib.rs".to_owned())).await;

    // Everything, with what's normally hidden marked, inside hidden folders too.
    apply_request(&mut client, &mut view, show_all(true)).await;
    apply_request(&mut client, &mut view, expand("node_modules", true)).await;
    assert_eq!(
        view.tree(),
        [
            "Docs/",
            "node_modules/ (hidden)",
            "  left-pad/ (hidden)",
            "src/",
            "  ui/",
            "    app.rs",
            "  lib.rs",
            "  main.rs",
            ".factoryignore",
            "README.md"
        ]
    );

    // Closing a folder closes the folders inside it; the project's own .factoryignore decides what's hidden.
    apply_request(&mut client, &mut view, expand("src", false)).await;
    apply_request(&mut client, &mut view, show_all(false)).await;
    std::fs::write(folder.join(".factoryignore"), "Docs/\n").expect("write");
    view.follow(&mut client, |view| view.tree() == ["node_modules/", "  left-pad/", "src/", ".factoryignore", "README.md"]).await;
    apply_request(&mut client, &mut view, expand("src", true)).await;
    assert_eq!(
        view.tree(),
        ["node_modules/", "  left-pad/", "src/", "  ui/", "  lib.rs", "  main.rs", ".factoryignore", "README.md"],
        "src/ui stayed closed"
    );

    for outside in ["../other", "/etc", "src/../.."] {
        match apply_request(&mut client, &mut view, expand(outside, true)).await {
            Body::Error(error) => assert_eq!(error.code(), ErrorCode::InvalidRequest, "{outside}"),
            other => panic!("{outside}: expected an error, got {other:?}"),
        }
    }

    // Focus and open folders are the workspace's, so a restart keeps them.
    let expected = view.tree();
    let (restarted, _factory) = start(&dir).await;
    let (mut other, workspace, _) = Client::connect(restarted).await;
    assert_eq!(workspace.focused_project_id, "site");
    let mut fresh = View { workspace, ..View::default() };
    fresh.follow(&mut other, |fresh| fresh.explorer.is_some()).await;
    assert_eq!(fresh.tree(), expected);

    // Leaving focus keeps the selection.
    apply_request(&mut other, &mut fresh, focus("")).await;
    assert_eq!((fresh.workspace.focused_project_id.as_str(), fresh.workspace.selected_agent_id.as_str()), ("", "writer"));
}

fn files_pane(agent: &str, project: &str) -> client_message::Body {
    let leaf = |id: &str, kind: &str, path: &str| v1::ViewNode {
        id: id.into(),
        node: Some(v1::view_node::Node::Leaf(v1::ViewLeaf { kind: kind.into(), path: path.into(), title: String::new() })),
    };
    let split = v1::ViewSplit {
        axis: v1::ViewAxis::Row.into(),
        children: vec![leaf("chat", "chat", ""), leaf("files", "files", project)],
        weights: vec![],
    };
    let root = v1::ViewNode { id: "split".into(), node: Some(v1::view_node::Node::Split(split)) };
    client_message::Body::SetLayout(v1::SetLayout { agent_id: agent.into(), root: Some(root) })
}

fn watch(project_id: &str) -> client_message::Body {
    client_message::Body::WatchFiles(v1::WatchFiles { project_id: project_id.into() })
}

/// The rows of one project's explorer, as `View::tree` writes them.
fn rows(view: &View, project: &str) -> Vec<String> {
    let explorer = view.explorers.get(project).cloned();
    View { explorer, ..View::default() }.tree()
}

#[tokio::test]
async fn a_files_pane_or_a_watching_connection_gets_that_projects_tree_too() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    apply_request(&mut client, &mut view, create_project("Site", Path::new(""))).await;
    apply_request(&mut client, &mut view, create_project("Blog", Path::new(""))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    let blog = dir.join("home/workspace/blog");
    std::fs::write(blog.join("post.md"), "").expect("write");
    apply_request(&mut client, &mut view, focus("site")).await;
    assert!(!view.explorers.contains_key("blog"), "only the focused project's files are open");

    // A Files pane of Blog in Writer's layout: Blog's tree comes, and Site's
    // again after it, for apps that keep only the last.
    apply_request(&mut client, &mut view, files_pane("writer", "blog")).await;
    view.follow(&mut client, |view| view.explorers.contains_key("blog")).await;
    assert_eq!(rows(&view, "blog"), [".factoryignore", "post.md"]);
    view.follow(&mut client, |view| view.explorer.as_ref().is_some_and(|explorer| explorer.project_id == "site")).await;

    // It follows the files, with its own open folders.
    std::fs::create_dir(blog.join("drafts")).expect("mkdir");
    std::fs::write(blog.join("drafts/next.md"), "").expect("write");
    view.follow(&mut client, |view| rows(view, "blog").contains(&"drafts/".to_owned())).await;
    let open = client_message::Body::ExpandFolder(v1::ExpandFolder {
        project_id: "blog".into(),
        path: "drafts".into(),
        expanded: true,
        folder_id: String::new(),
        agent_id: String::new(),
    });
    apply_request(&mut client, &mut view, open).await;
    view.follow(&mut client, |view| rows(view, "blog") == ["drafts/", "  next.md", ".factoryignore", "post.md"]).await;
    assert!(rows(&view, "site").iter().all(|row| !row.starts_with("  ")), "Site's folders stay as they were");

    // On connect, every open project's tree comes, the focused one's last.
    let (mut other, _, _) = Client::connect(addr).await;
    let mut fresh = View::default();
    fresh.follow(&mut other, |fresh| fresh.explorers.len() == 2).await;
    assert_eq!(fresh.explorer.as_ref().map(|explorer| explorer.project_id.as_str()), Some("site"));

    // Without the pane, only a connection watching Blog gets it.
    let plain = client_message::Body::SetLayout(v1::SetLayout { agent_id: "writer".into(), root: None });
    apply_request(&mut client, &mut view, plain).await;
    match apply_request(&mut other, &mut fresh, watch("nowhere")).await {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::NotFound),
        other => panic!("expected an error, got {other:?}"),
    }
    assert!(matches!(apply_request(&mut other, &mut fresh, watch("blog")).await, Body::Ack(_)));
    std::fs::write(blog.join("about.md"), "").expect("write");
    fresh.follow(&mut other, |fresh| rows(fresh, "blog").contains(&"about.md".to_owned())).await;

    // Once it stops watching (or leaves), Blog's files aren't open anywhere.
    drop(other);
    let closed = async {
        loop {
            let (mut probe, _, _) = Client::connect(addr).await;
            let mut seen = View::default();
            seen.follow(&mut probe, |seen| seen.explorers.contains_key("site")).await;
            if !seen.explorers.contains_key("blog") {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), closed).await.expect("Blog's files close when no one shows them");
}
