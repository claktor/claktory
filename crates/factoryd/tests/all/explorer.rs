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
