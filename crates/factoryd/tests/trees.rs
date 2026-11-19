//! Own checkout: an agent with the "tree" extension works in a checkout of
//! its project's repository of its own, can't change the main checkout with
//! its file tools, lands its commits with tree_land, and its checkout goes
//! with it when nothing would be lost.

mod common;

use std::path::Path;
use std::process::Command;

use common::{Client, View, apply_request, create_agent, create_project, describe, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// What the agent's call of `tool` with `args` answered.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

#[tokio::test]
async fn an_agent_works_in_its_own_checkout_and_lands_its_work() {
    let dir = temp_dir();
    let repo = dir.join("site");
    std::fs::create_dir_all(repo.join("target")).expect("repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), "target/\n.factoryignore\n").expect("ignore");
    std::fs::write(repo.join("lib.rs"), "one\n").expect("lib");
    std::fs::write(repo.join("target/built"), "warm").expect("build output");
    git(&repo, &["add", "."]);
    git(&repo, &["-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]);

    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &repo)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let tree_on = v1::ExtensionChange { id: "tree".into(), enabled: Some(true), ..Default::default() };
    let update = v1::UpdateAgent { agent_id: "builder".into(), extension: Some(tree_on), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(update)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;

    // Its first run makes the checkout, and it works there.
    let tree = dir.join("home/trees/builder/site");
    let here = call(&mut client, &mut view, "builder", "bash", r#"{"command":"pwd; ls target || echo no build output"}"#).await;
    assert!(here.contains("home/trees/builder/site\n"), "{here}");
    assert!(here.contains("no build output"), "only what git tracks: {here}");
    let notices: Vec<String> = describe(&view.entries).into_iter().filter(|line| line.starts_with("notice")).collect();
    assert!(notices[0].starts_with("notice: Making Builder's own checkout of "), "{notices:?}");
    assert!(notices[0].ends_with("Builder starts as soon as it's ready."), "{notices:?}");
    assert!(notices[1].starts_with("notice: Builder's own checkout is ready: ") && notices[1].ends_with("home/trees/builder/site."), "{notices:?}");
    assert_eq!(git(&tree, &["branch", "--show-current"]), "home/builder");

    // The main checkout is only to read.
    let main_file = repo.join("lib.rs").display().to_string();
    let refused = call(&mut client, &mut view, "builder", "write_file", &format!(r#"{{"path":"{main_file}","content":"mine"}}"#)).await;
    assert!(refused.contains("read only"), "{refused}");

    // It commits in its checkout and lands the work on main.
    call(&mut client, &mut view, "builder", "write_file", r#"{"path":"lib.rs","content":"two\n"}"#).await;
    call(&mut client, &mut view, "builder", "bash", r#"{"command":"git -c user.name=T -c user.email=t@t commit -qam work"}"#).await;
    let landed = call(&mut client, &mut view, "builder", "tree_land", "{}").await;
    assert!(landed.starts_with("Landed 1 commit on main"), "{landed}");
    assert_eq!(std::fs::read_to_string(repo.join("lib.rs")).expect("landed"), "two\n");

    // The explorer shows its checkout.
    apply_request(&mut client, &mut view, client_message::Body::FocusProject(v1::FocusProject { project_id: "site".into() })).await;
    view.follow(&mut client, |view| {
        view.explorer.as_ref().is_some_and(|explorer| explorer.folders.iter().any(|root| !root.agent_id.is_empty()))
    })
    .await;
    let roots = &view.explorer.as_ref().expect("explorer").folders;
    let root = roots.iter().find(|root| root.agent_id == "builder").expect("its checkout");
    assert_eq!((root.branch.as_str(), root.folder_id.as_str()), ("home/builder", ""));
    assert!(root.entries.iter().any(|entry| entry.name == "lib.rs"), "{:?}", root.entries);

    // Deleted, with everything landed: its checkout and branch go.
    apply_request(&mut client, &mut view, client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "builder".into() })).await;
    for _ in 0..500 {
        if !tree.exists() && git(&repo, &["branch", "--list", "home/builder"]).is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!tree.exists(), "the checkout went");
    assert_eq!(git(&repo, &["branch", "--list", "home/builder"]), "");
}

#[tokio::test]
async fn a_checkout_with_work_not_landed_is_kept_when_the_extension_goes_off() {
    let dir = temp_dir();
    let repo = dir.join("site");
    std::fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("lib.rs"), "one\n").expect("lib");
    git(&repo, &["add", "."]);
    git(&repo, &["-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]);

    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &repo)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let switch = |on: bool| {
        let change = v1::ExtensionChange { id: "tree".into(), enabled: Some(on), ..Default::default() };
        client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: "builder".into(), extension: Some(change), ..Default::default() })
    };
    apply_request(&mut client, &mut view, switch(true)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    call(&mut client, &mut view, "builder", "bash", r#"{"command":"echo wip > wip.txt"}"#).await;
    let tree = dir.join("home/trees/builder/site");
    assert!(tree.join("wip.txt").exists());

    // Turned off: it's back in the project's folder, told so, and the
    // checkout with its uncommitted work stays.
    apply_request(&mut client, &mut view, switch(false)).await;
    let back = call(&mut client, &mut view, "builder", "bash", r#"{"command":"pwd"}"#).await;
    assert!(back.starts_with('/') && back.lines().next().is_some_and(|line| line.ends_with("/site")) && !back.contains("trees"), "{back}");
    let told = describe(&view.entries).into_iter().rev().find(|line| line.contains("Your settings changed")).expect("told");
    assert!(told.contains("the Own checkout extension is now off") && told.contains("now start in"), "{told}");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(tree.join("wip.txt").exists(), "kept: it has uncommitted work");

    // On again: the same checkout, and it's told where it works.
    apply_request(&mut client, &mut view, switch(true)).await;
    let again = call(&mut client, &mut view, "builder", "bash", r#"{"command":"cat wip.txt"}"#).await;
    assert!(again.starts_with("wip"), "{again}");
    let told = describe(&view.entries).into_iter().rev().find(|line| line.contains("Your settings changed")).expect("told");
    assert!(told.contains("## Own checkout") && told.contains("on the branch home/builder"), "{told}");
}
