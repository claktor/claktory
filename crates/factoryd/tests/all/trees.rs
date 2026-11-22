//! Own checkout: an agent with the "tree" extension works in a checkout of
//! its project's repository of its own, can't change the main checkout with
//! its file tools, lands its commits with tree_land, and its checkout goes
//! with it when nothing would be lost.

use std::path::Path;
use std::process::Command;

use crate::common::{Client, View, apply_request, create_agent, create_project, describe, send_message, start, temp_dir};
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
    assert!(
        notices[1].starts_with("notice: Builder's own checkout is ready: ") && notices[1].ends_with("home/trees/builder/site."),
        "{notices:?}"
    );
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

/// Commits everything in `dir`, as `message`.
fn commit_all(dir: &Path, message: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "-m", message]);
}

/// c9a8000c: a branch squashed onto main with `git reset --soft main` from a
/// checkout behind main puts back every file main changed meanwhile, in its
/// own commit, where a rebase can't see it. tree_land refuses it and says
/// how to squash instead; a revert meant as one lands with allow_rewinds.
#[tokio::test]
async fn a_landing_that_puts_others_work_back_is_refused_unless_meant() {
    let dir = temp_dir();
    let repo = dir.join("site");
    std::fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    for (name, text) in [("a.txt", "a1\n"), ("b.txt", "b1\n"), ("c.txt", "c1\n")] {
        std::fs::write(repo.join(name), text).expect("write");
    }
    commit_all(&repo, "start");

    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &repo)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let tree_on = v1::ExtensionChange { id: "tree".into(), enabled: Some(true), ..Default::default() };
    let update = v1::UpdateAgent { agent_id: "builder".into(), extension: Some(tree_on), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(update)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let bash = |command: &str| format!(r#"{{"command":{}}}"#, serde_json::to_string(command).expect("json"));

    // Builder's work, in two commits on the old main.
    let work = "echo mine > mine.txt && git add -A && git -c user.name=T -c user.email=t@t commit -qm one && echo more >> mine.txt && git -c user.name=T -c user.email=t@t commit -qam two";
    call(&mut client, &mut view, "builder", "bash", &bash(work)).await;
    // Meanwhile others land: a.txt changes, new.txt comes, c.txt goes.
    std::fs::write(repo.join("a.txt"), "a2\n").expect("write");
    std::fs::write(repo.join("new.txt"), "new\n").expect("write");
    std::fs::remove_file(repo.join("c.txt")).expect("remove");
    commit_all(&repo, "others");
    let main_before = git(&repo, &["rev-parse", "HEAD"]);

    // Squashed onto the new main from the old tree: refused, naming exactly
    // the files it would put back, and main is as it was.
    let squash = "git reset -q --soft main && git -c user.name=T -c user.email=t@t commit -qm squashed";
    call(&mut client, &mut view, "builder", "bash", &bash(squash)).await;
    let refused = call(&mut client, &mut view, "builder", "tree_land", "{}").await;
    assert!(refused.contains("Not landed: your branch would put 3 files back to a version main had before"), "{refused}");
    assert!(refused.contains("- a.txt\n- c.txt\n- new.txt\n"), "{refused}");
    assert!(!refused.contains("mine.txt") && !refused.contains("b.txt"), "{refused}");
    assert!(refused.contains("git reset --soft $(git merge-base HEAD main)"), "{refused}");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), main_before);

    // Squashed as it says, then rebased: it lands, and others' work stays.
    let redo = "git reset -q --hard HEAD@{2} && git reset -q --soft $(git merge-base HEAD main) && git -c user.name=T -c user.email=t@t commit -qm squashed && git rebase -q main";
    call(&mut client, &mut view, "builder", "bash", &bash(redo)).await;
    let landed = call(&mut client, &mut view, "builder", "tree_land", "{}").await;
    assert!(landed.starts_with("Landed 1 commit on main"), "{landed}");
    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "a2\n");
    assert!(repo.join("new.txt").exists() && !repo.join("c.txt").exists());
    assert_eq!(std::fs::read_to_string(repo.join("mine.txt")).unwrap(), "mine\nmore\n");

    // A revert meant as one: refused until it says so.
    let revert = format!("git -c user.name=T -c user.email=t@t revert --no-edit {main_before}");
    call(&mut client, &mut view, "builder", "bash", &bash(&revert)).await;
    let refused = call(&mut client, &mut view, "builder", "tree_land", "{}").await;
    assert!(refused.contains("put 3 files back") && refused.contains("allow_rewinds: true"), "{refused}");
    let landed = call(&mut client, &mut view, "builder", "tree_land", r#"{"allow_rewinds":true}"#).await;
    assert!(landed.starts_with("Landed 1 commit on main"), "{landed}");
    assert_eq!(std::fs::read_to_string(repo.join("a.txt")).unwrap(), "a1\n");
}
