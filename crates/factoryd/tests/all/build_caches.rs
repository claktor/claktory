//! Build caches stay bounded on their own: over the budget, an agent's
//! checkout's caches (a Rust target/, node_modules/…) and a project folder's
//! are cleared while nothing works there, and the agent hears of it at its
//! next run. Nothing else git ignores is ever touched.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// Writes `bytes` to `path` and makes everything on the way look an hour
/// old, as a build that finished a while ago leaves it.
fn built(path: &Path, bytes: usize) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, vec![7; bytes]).expect("write");
    let old = SystemTime::now() - Duration::from_secs(3600);
    for at in path.ancestors().take(4) {
        if let Ok(file) = std::fs::File::open(at) {
            let _ = file.set_modified(old);
        }
    }
}

async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what} never happened");
}

/// A repository with a Rust crate, a project on it and its agent Builder,
/// whose checkout is made by a first run; the budget is 1 MB.
async fn setup(dir: &Path) -> (std::sync::Arc<factoryd::Factory>, Client, View, PathBuf, PathBuf) {
    let repo = dir.join("site");
    std::fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join(".gitignore"), "target/\ndata/\nbuild/\n.env\n").expect("ignore");
    std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"site\"\n").expect("cargo");
    git(&repo, &["add", "."]);
    git(&repo, &["-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "-m", "start"]);

    let (addr, factory) = start(dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &repo)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let tree_on = v1::ExtensionChange { id: "tree".into(), enabled: Some(true), ..Default::default() };
    let update = v1::UpdateAgent { agent_id: "builder".into(), extension: Some(tree_on), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(update)).await;
    let budget = v1::UpdateSettings { build_cache_gb: Some(0.001), ..Default::default() };
    let reply = apply_request(&mut client, &mut view, client_message::Body::UpdateSettings(budget)).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(view.workspace.settings.as_ref().map(|settings| settings.build_cache_gb), Some(0.001));
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    let tree = dir.join("home/trees/builder/site");
    assert!(tree.join(".git").is_file(), "the checkout was made");
    (factory, client, view, repo, tree)
}

/// What Builder was told by Factory of cleared caches.
fn told(view: &View) -> Vec<String> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory && event.text.contains("build cache") => {
                Some(event.text.clone())
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn caches_over_budget_are_cleared_while_idle_and_nothing_else_is_touched() {
    let dir = temp_dir();
    let (factory, mut client, mut view, repo, tree) = setup(&dir).await;
    // Builds in the checkout and in the main folder; things git ignores that
    // aren't caches: data/, a build/ with no Flutter project, .env.
    built(&tree.join("target/debug/big"), 2_000_000);
    built(&repo.join("target/debug/big"), 2_000_000);
    built(&tree.join("data/keep.csv"), 3_000_000);
    built(&tree.join("build/out.html"), 3_000_000);
    built(&tree.join(".env"), 10);
    // Under the budget: kept.
    std::fs::write(tree.join("package.json"), "{}").expect("package");
    built(&tree.join("node_modules/tiny/index.js"), 1_000);

    factory.set_build_cache_every(Duration::from_millis(100));
    eventually("clearing the checkout's target", || !tree.join("target").exists()).await;
    eventually("clearing the main folder's target", || !repo.join("target").exists()).await;
    for kept in ["data/keep.csv", "build/out.html", ".env", "node_modules/tiny/index.js", "Cargo.toml"] {
        assert!(tree.join(kept).exists(), "{kept} is left");
    }
    let trash = dir.join("home/trash");
    eventually("deleting what was moved aside", || std::fs::read_dir(&trash).is_ok_and(|mut entries| entries.next().is_none())).await;

    // Builder hears of it at its next run, not before.
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    assert!(told(&view).is_empty(), "an idle agent isn't woken for it");
    apply_request(&mut client, &mut view, send_message("builder", "build it")).await;
    view.follow(&mut client, |view| idle(view, "builder") && !told(view).is_empty()).await;
    let told = told(&view);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(
        told[0].starts_with("Your checkout's build cache (0.0 GB, ") && told[0].ends_with("was cleared; the next build is cold."),
        "{told:?}"
    );
}

#[tokio::test]
async fn caches_are_left_while_the_agent_works_and_cleared_on_request() {
    let dir = temp_dir();
    let (factory, mut client, mut view, _repo, tree) = setup(&dir).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;

    // A background task of Builder's runs: its checkout is left alone.
    let task = r#"call bash {"command":"sleep 30","run_in_background":true}"#;
    apply_request(&mut client, &mut view, send_message("builder", task)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    built(&tree.join("target/debug/big"), 2_000_000);
    factory.set_build_cache_every(Duration::from_millis(50));
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(tree.join("target/debug/big").exists(), "left while Builder's task runs");

    // Asked to clear it meanwhile, it says why not.
    let clear = |path: &Path| client_message::Body::ClearBuildCache(v1::ClearBuildCache { path: path.display().to_string() });
    let refused = apply_request(&mut client, &mut view, clear(&tree.join("target"))).await;
    assert!(
        matches!(&refused, Body::Error(error) if error.message == "Builder is working there now; clear it once Builder is idle."),
        "{refused:?}"
    );

    // Once the task is stopped, it's cleared.
    let stop = v1::StopTask { agent_id: "builder".into(), task_id: "task-1".into() };
    apply_request(&mut client, &mut view, client_message::Body::StopTask(stop)).await;
    eventually("clearing after the task ended", || !tree.join("target").exists()).await;

    // On request: a cache under the budget is cleared; what isn't a cache isn't.
    // Builder first hears its task ended (the clearing may come before,
    // during or after that run), and is idle again.
    factory.set_build_cache_every(Duration::from_secs(3600));
    let heard = |view: &View| {
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.source() == v1::EventSource::Task))
    };
    view.follow(&mut client, |view| heard(view) && idle(view, "builder")).await;
    std::fs::write(tree.join("package.json"), "{}").expect("package");
    built(&tree.join("node_modules/tiny/index.js"), 1_000);
    built(&tree.join("data/keep.csv"), 1_000);
    let reply = apply_request(&mut client, &mut view, clear(&tree.join("node_modules"))).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert!(!tree.join("node_modules").exists());
    let refused = apply_request(&mut client, &mut view, clear(&tree.join("data"))).await;
    assert!(matches!(&refused, Body::Error(error) if error.message.contains("isn't a build cache Claktory knows")), "{refused:?}");
    assert!(tree.join("data/keep.csv").exists());
    let outside = apply_request(&mut client, &mut view, clear(&dir.join("elsewhere/target"))).await;
    assert!(
        matches!(&outside, Body::Error(error) if error.message.contains("isn't in an agent's checkout or a project's folder")),
        "{outside:?}"
    );
}

#[tokio::test]
async fn the_dev_claktory_keeps_to_its_own_data_folder() {
    // A dev Claktory, with a project in its data folder and one outside it,
    // such as the repository the real Claktory's agents build in.
    let dir = temp_dir();
    let home = factoryd::home::Home::new(dir.join(".factory-dev"));
    let factory = factoryd::Factory::open(home.clone(), std::sync::Arc::new(crate::common::Scripted::signed_in())).await.expect("open");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    crate::common::serves(addr, home);
    tokio::spawn(factoryd::serve(listener, factory.clone()));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let (inside, outside) = (dir.join(".factory-dev/workspace/lab"), dir.join("factory"));
    for (name, path) in [("Lab", &inside), ("Factory", &outside)] {
        std::fs::create_dir_all(path).expect("project");
        std::fs::write(path.join("Cargo.toml"), "[package]\nname = \"x\"\n").expect("cargo");
        built(&path.join("target/debug/big"), 2_000_000);
        apply_request(&mut client, &mut view, create_project(name, path)).await;
    }
    let budget = v1::UpdateSettings { build_cache_gb: Some(0.001), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateSettings(budget)).await;

    factory.set_build_cache_every(Duration::from_millis(100));
    eventually("clearing the target in its data folder", || !inside.join("target").exists()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(outside.join("target/debug/big").exists(), "a folder outside its data folder is left");

    // Asked to, it says why not.
    let clear = client_message::Body::ClearBuildCache(v1::ClearBuildCache { path: outside.join("target").display().to_string() });
    let refused = apply_request(&mut client, &mut view, clear).await;
    assert!(matches!(&refused, Body::Error(error) if error.message.contains("outside this Claktory's data folder")), "{refused:?}");
    assert!(outside.join("target/debug/big").exists());
}

#[tokio::test]
async fn a_busy_agent_s_caches_are_trimmed_between_its_steps_and_cleared_once_idle_never_in_a_tool_call() {
    let dir = temp_dir();
    let (factory, mut client, mut view, repo, tree) = setup(&dir).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;

    // In a tool call (a build, say): left, however long it runs. The call
    // says when it has started, and runs until the test lets it end, so
    // nothing here depends on how fast the machine is.
    let (started, release) = (dir.join("call-started"), dir.join("call-release"));
    let build =
        format!(r#"call bash {{"command":"touch {}; while [ ! -e {} ]; do sleep 0.05; done"}}"#, started.display(), release.display());
    apply_request(&mut client, &mut view, send_message("builder", &build)).await;
    eventually("Builder's tool call starting", || started.exists()).await;
    built(&tree.join("target/debug/big"), 2_000_000);
    factory.set_build_cache_every(Duration::from_millis(50));
    // Sweeps that ran in full while the call runs: each clears a cache over
    // budget in the project's folder, where no one works. The second is
    // cleared by a sweep that began after the first was, so after Builder's
    // cache was there.
    for _ in 0..2 {
        built(&repo.join("target/debug/big"), 2_000_000);
        eventually("a sweep while Builder's tool call runs", || !repo.join("target").exists()).await;
    }
    assert!(tree.join("target/debug/big").exists(), "left while Builder's tool call runs");
    std::fs::write(&release, "").expect("release");
    view.follow(&mut client, |view| idle(view, "builder")).await;
    eventually("clearing once the call ended", || !tree.join("target").exists()).await;
    factory.set_build_cache_every(Duration::from_secs(3600));
    apply_request(&mut client, &mut view, send_message("builder", "build again")).await;
    view.follow(&mut client, |view| idle(view, "builder") && told(view).len() == 1).await;

    // Working, between steps (waiting on its model): a Rust target is
    // trimmed (its incremental state goes, unannounced) but not cleared,
    // since its next build would be cold.
    apply_request(&mut client, &mut view, send_message("builder", "hang")).await;
    view.follow(&mut client, |view| view.agent("builder").status() == AgentStatus::Working).await;
    rust_build(&tree.join("target"));
    // Written just now: left, between steps too.
    std::fs::write(tree.join("package.json"), "{}").expect("package");
    std::fs::create_dir_all(tree.join("node_modules/fresh")).expect("fresh");
    std::fs::write(tree.join("node_modules/fresh/index.js"), vec![7; 2_000_000]).expect("fresh");
    factory.set_build_cache_every(Duration::from_millis(50));
    eventually("trimming between steps", || !tree.join("target/debug/incremental").exists()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(tree.join("target/debug/big").exists(), "not cleared while Builder works");
    assert!(tree.join("node_modules/fresh/index.js").exists(), "a cache written in the last minutes is left");

    // Once Builder is idle, still over the budget: cleared, and it hears of
    // it at its next run.
    apply_request(&mut client, &mut view, client_message::Body::StopAgent(v1::StopAgent { agent_id: "builder".into() })).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    eventually("clearing once Builder is idle", || !tree.join("target").exists()).await;
    factory.set_build_cache_every(Duration::from_secs(3600));
    apply_request(&mut client, &mut view, send_message("builder", "go on")).await;
    view.follow(&mut client, |view| idle(view, "builder") && told(view).len() == 2).await;
    let told = told(&view);
    assert_eq!(told.len(), 2, "the trim wasn't announced, the clearing was: {told:?}");
    assert!(told[1].starts_with("Your checkout's build cache (") && told[1].ends_with("was cleared; the next build is cold."), "{told:?}");
}

/// A Rust build's target, an hour old: 2 MB of incremental state and 2 MB
/// of the build itself.
fn rust_build(target: &Path) {
    built(&target.join("debug/incremental/site-1/s-1/dep-graph.bin"), 2_000_000);
    built(&target.join("debug/big"), 2_000_000);
    built(&target.join(".rustc_info.json"), 10);
}

/// Over the budget, a Rust target loses its incremental state first, and is
/// left once that's enough: the next build there isn't a cold one.
#[tokio::test]
async fn a_rust_target_is_trimmed_before_it_is_cleared() {
    let dir = temp_dir();
    let (factory, mut client, mut view, _repo, tree) = setup(&dir).await;
    let target = tree.join("target");
    built(&target.join("debug/incremental/site-1/s-1/dep-graph.bin"), 2_000_000);
    built(&target.join("debug/small"), 100_000);
    built(&target.join(".rustc_info.json"), 10);
    factory.set_build_cache_every(Duration::from_millis(50));
    eventually("trimming the target", || !target.join("debug/incremental").exists()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(target.join("debug/small").exists(), "under the budget once trimmed: kept");
    factory.set_build_cache_every(Duration::from_secs(3600));
    apply_request(&mut client, &mut view, send_message("builder", "build it")).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;
    assert!(told(&view).is_empty(), "a trim isn't announced: {:?}", told(&view));
}

/// A busy agent nearly always has a background task or a watch running, and
/// its checkout grows most: a Rust target there is trimmed all the same once
/// it's quiet, but cleared whole only once nothing runs (Polpo's disk.md,
/// item 1).
#[tokio::test]
async fn a_rust_target_is_trimmed_while_its_agent_s_task_runs_but_cleared_only_after() {
    let dir = temp_dir();
    let (factory, mut client, mut view, _repo, tree) = setup(&dir).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let task = r#"call bash {"command":"sleep 30","run_in_background":true}"#;
    apply_request(&mut client, &mut view, send_message("builder", task)).await;
    view.follow(&mut client, |view| idle(view, "builder")).await;

    // Written just now (a build between two of its steps): left.
    let target = tree.join("target");
    built(&target.join(".rustc_info.json"), 10);
    built(&target.join("debug/big"), 2_000_000);
    std::fs::create_dir_all(target.join("debug/incremental/site-1/s-1")).expect("mkdir");
    std::fs::write(target.join("debug/incremental/site-1/s-1/dep-graph.bin"), vec![7; 2_000_000]).expect("fresh");
    factory.set_build_cache_every(Duration::from_millis(50));
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(target.join("debug/incremental").exists(), "a target written in the last two minutes isn't trimmed");

    // Quiet: trimmed while the task runs, and the rest left.
    factory.set_build_cache_every(Duration::from_secs(3600));
    rust_build(&target);
    factory.set_build_cache_every(Duration::from_millis(50));
    eventually("trimming while Builder's task runs", || !target.join("debug/incremental").exists()).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(target.join("debug/big").exists(), "not cleared while Builder's task runs");

    // Once the task is stopped, still over the budget: cleared.
    let stop = v1::StopTask { agent_id: "builder".into(), task_id: "task-1".into() };
    apply_request(&mut client, &mut view, client_message::Body::StopTask(stop)).await;
    eventually("clearing after the task ended", || !target.exists()).await;
}
