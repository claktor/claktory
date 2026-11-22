//! `factoryd export` (#217): a Claktory into a move bundle (docs/MOVE.md),
//! and with --final the tombstone, which --restore undoes and
//! --delete-moved finishes.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::AgentStatus;
use factoryd::home::Home;
use factoryd::move_bundle;
use factoryd::move_export::{self, Options, Place, Service};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .expect("git");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository with history, an uncommitted change, an untracked file and
/// ignored build output.
fn repository(path: &Path) {
    std::fs::create_dir_all(path).expect("mkdir");
    git(path, &["init"]);
    std::fs::write(path.join(".gitignore"), "target/\n").expect("write");
    std::fs::write(path.join("README.md"), "first\n").expect("write");
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "first"]);
    std::fs::write(path.join("README.md"), "first\nchanged, not committed\n").expect("write");
    std::fs::write(path.join("notes.txt"), "untracked\n").expect("write");
    std::fs::create_dir_all(path.join("target/debug")).expect("mkdir");
    std::fs::write(path.join("target/debug/huge.bin"), vec![0u8; 4096]).expect("write");
}

/// Builder's own checkout, as Claktory made it, with work of its own
/// committed and not. Returns its branch.
fn checkout(home: &Path) -> String {
    let tree = home.join("trees/builder/site");
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("trees/builder/site.json")).expect("Claktory made the checkout"))
            .expect("its record");
    std::fs::write(tree.join("feature.rs"), "fn feature() {}\n").expect("write");
    git(&tree, &["add", "."]);
    git(&tree, &["commit", "-m", "feature"]);
    std::fs::write(tree.join("feature.rs"), "fn feature() { todo!() }\n").expect("write");
    std::fs::write(tree.join("scratch.md"), "untracked in the checkout\n").expect("write");
    record["branch"].as_str().expect("a branch").to_owned()
}

fn place(dir: &Path, port: u16) -> Place {
    Place { home: Home::new(dir.join("home")), port, user_home: dir.to_path_buf(), dev: false, service: Service::None }
}

/// A running Claktory with the project Site (a git repository outside its
/// home), its agent Builder, a conversation, and things that never move.
async fn claktory(dir: &Path) -> (std::sync::Arc<factoryd::Factory>, std::net::SocketAddr, PathBuf) {
    let repo = dir.join("site");
    repository(&repo);
    let (addr, factory) = start(dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &repo)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| view.agent("builder").status() == AgentStatus::Idle && view.agent("builder").queued_messages == 0)
        .await;
    let home = dir.join("home");
    assert_eq!(checkout(&home), "home/builder");
    std::fs::write(home.join("auth.json"), "{\"scripted\": {}}").expect("write");
    for left_out in ["bin", "releases", "models/speech", "trash"] {
        std::fs::create_dir_all(home.join(left_out)).expect("mkdir");
        std::fs::write(home.join(left_out).join("big"), "not moved").expect("write");
    }
    std::fs::write(home.join("factoryd.log"), "log").expect("write");
    (factory, addr, repo)
}

#[tokio::test]
async fn a_live_export_carries_the_claktory_its_folders_and_checkouts_and_changes_nothing() {
    let dir = temp_dir();
    let (_factory, _addr, repo) = claktory(&dir).await;
    let out = dir.join("out/site.claktory");
    std::fs::create_dir_all(out.parent().unwrap()).expect("mkdir");
    let options = Options { out: Some(out.clone()), ..Options::default() };

    let exported = move_export::export(&place(&dir, 0), &options).expect("exported");
    assert_eq!(exported.bundle, out);
    assert!(out.is_file());
    let leftovers: Vec<_> = std::fs::read_dir(out.parent().unwrap()).unwrap().map(|entry| entry.unwrap().file_name()).collect();
    assert_eq!(leftovers, [std::ffi::OsString::from("site.claktory")], "nothing staged is left");
    assert!(!dir.join("home/moved.json").exists() && dir.join("home/auth.json").exists(), "a plain export changes nothing");

    let manifest = &exported.manifest;
    assert!(!manifest.is_final);
    assert_eq!(manifest.epoch, 1);
    assert_eq!(manifest.source.home, dir.join("home").display().to_string());
    assert_eq!(manifest.folders.len(), 1, "{:?}", manifest.folders);
    let folder = &manifest.folders[0];
    assert_eq!((folder.path.as_str(), folder.kind.as_str(), folder.branch.as_str()), (repo.to_str().unwrap(), "git", "main"));
    assert_eq!(folder.projects, ["site"]);
    assert_eq!(folder.head, git(&repo, &["rev-parse", "HEAD"]));
    assert_eq!(manifest.checkouts.len(), 1);
    let checkout = &manifest.checkouts[0];
    assert_eq!((checkout.agent.as_str(), checkout.branch.as_str()), ("builder", "home/builder"));
    assert!(manifest.requires.disk_bytes > manifest.bytes);

    // Unpacked, it's all there.
    let unpacked = dir.join("unpacked");
    let read = move_bundle::unpack(&out, &exported.passphrase, &unpacked).expect("unpacked");
    assert_eq!(&read, manifest);
    let home = unpacked.join("home");
    assert!(home.join("config.json").is_file() && home.join("auth.json").is_file());
    let db = rusqlite::Connection::open(home.join("factory.db")).expect("a database");
    let sent: String = db.query_row("SELECT text FROM messages WHERE agent = 'builder'", [], |row| row.get(0)).expect("the message");
    assert_eq!(sent, "hello");
    assert!(home.join("vault/vault.db").is_file(), "the vault");
    assert!(
        std::fs::read_dir(home.join("agents/builder")).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("segment-"))
    );
    for gone in ["bin", "releases", "models", "trash", "trees", "factoryd.log", "factory.db-wal", "factory.db-shm"] {
        assert!(!home.join(gone).exists(), "{gone} isn't moved");
    }

    // The repository comes back from its bundle, with the agent's branch,
    // and its uncommitted work applies on top.
    let folder_dir = unpacked.join("folders/0");
    let clone = dir.join("clone");
    git(&dir, &["clone", "-q", folder_dir.join("repo.bundle").to_str().unwrap(), clone.to_str().unwrap()]);
    git(&clone, &["apply", folder_dir.join("changes.patch").to_str().unwrap()]);
    assert_eq!(std::fs::read_to_string(clone.join("README.md")).unwrap(), "first\nchanged, not committed\n");
    assert_eq!(std::fs::read_to_string(folder_dir.join("untracked/notes.txt")).unwrap(), "untracked\n");
    assert!(!folder_dir.join("untracked/target").exists(), "ignored build output isn't moved");
    let branches = git(&clone, &["branch", "-r"]);
    assert!(branches.contains("origin/home/builder"), "{branches}");
    let checkout_dir = unpacked.join("checkouts/builder/site");
    git(&clone, &["checkout", "-q", "-b", "home/builder", "origin/home/builder"]);
    git(&clone, &["apply", checkout_dir.join("changes.patch").to_str().unwrap()]);
    assert_eq!(std::fs::read_to_string(clone.join("feature.rs")).unwrap(), "fn feature() { todo!() }\n");
    assert!(checkout_dir.join("untracked/scratch.md").is_file());
}

#[tokio::test]
async fn a_final_export_fences_the_old_copy_which_comes_back_with_restore_or_goes_with_delete() {
    let dir = temp_dir();
    let (factory, _addr, _repo) = claktory(&dir).await;
    drop(factory);
    let home = dir.join("home");
    let place = place(&dir, 0);
    let options = Options { is_final: true, out: Some(dir.join("move.claktory")), ..Options::default() };

    let exported = move_export::export(&place, &options).expect("moved");
    assert!(exported.manifest.is_final);
    move_bundle::check(&exported.bundle, &exported.passphrase).expect("it reads back");
    // Tombstoned, with nothing deleted: what lets it run is put away.
    let moved = move_export::read_moved(&Home::new(home.clone())).expect("moved.json");
    assert_eq!((moved.export_id.as_str(), moved.epoch), (exported.manifest.export_id.as_str(), 1));
    for fenced in ["auth.json", "vault", "remote"] {
        assert!(!home.join(fenced).exists() && home.join("moved").join(fenced).exists(), "{fenced} is fenced");
    }
    assert!(home.join("agents/builder").is_dir() && home.join("factory.db").is_file(), "the data stays");

    let again = move_export::export(&place, &Options { out: Some(dir.join("again.claktory")), ..Options::default() });
    assert!(again.unwrap_err().reason.starts_with("This Claktory already moved"));

    // Brought back, as it was.
    move_export::restore(&place).expect("restored");
    assert!(move_export::read_moved(&Home::new(home.clone())).is_none());
    for fenced in ["auth.json", "vault", "remote"] {
        assert!(home.join(fenced).exists(), "{fenced} is back");
    }
    assert_eq!(std::fs::read_to_string(home.join("auth.json")).unwrap(), "{\"scripted\": {}}");

    // Moved again, then deleted for good: the workspace stays, and it keeps saying it moved.
    let options = Options { is_final: true, out: Some(dir.join("move-2.claktory")), ..Options::default() };
    let second = move_export::export(&place, &options).expect("moved again");
    assert_eq!(second.manifest.epoch, 1, "the epoch is the target's to keep");
    std::fs::create_dir_all(home.join("workspace/notes")).expect("mkdir");
    let deleted = move_export::deleted_by(&place);
    assert!(deleted.contains(&home.join("agents")) && deleted.contains(&home.join("moved")));
    assert!(!deleted.iter().any(|path| path.ends_with("workspace") || path.ends_with("moved.json") || path.ends_with("bin")));
    move_export::delete_moved(&place).expect("deleted");
    assert!(!home.join("agents").exists() && !home.join("factory.db").exists() && !home.join("moved").exists());
    assert!(home.join("workspace/notes").is_dir() && home.join("bin").is_dir());
    assert!(move_export::read_moved(&Home::new(home.clone())).expect("still moved").deleted_ms > 0);
    assert!(move_export::restore(&place).is_err(), "a deleted copy can't come back");
}

#[tokio::test]
async fn an_export_that_cant_go_says_why_and_changes_nothing() {
    let dir = temp_dir();
    let (_factory, addr, repo) = claktory(&dir).await;
    let home = dir.join("home");

    // Running, with nothing this can stop.
    let running = Options { is_final: true, out: Some(dir.join("a.claktory")), ..Options::default() };
    let refused = move_export::export(&place(&dir, addr.port()), &running).unwrap_err();
    assert!(refused.reason.contains("is running"), "{}", refused.reason);

    // Inside Claktory's folder, or inside a folder it carries.
    let inside = Options { out: Some(home.join("workspace/b.claktory")), ..Options::default() };
    assert!(move_export::export(&place(&dir, 0), &inside).unwrap_err().reason.contains("Claktory's own folder"));
    let carried = Options { out: Some(repo.join("c.claktory")), ..Options::default() };
    assert!(move_export::export(&place(&dir, 0), &carried).unwrap_err().reason.contains("which it carries"));

    // Already there.
    std::fs::write(dir.join("d.claktory"), "taken").expect("write");
    let taken = Options { out: Some(dir.join("d.claktory")), ..Options::default() };
    assert!(move_export::export(&place(&dir, 0), &taken).unwrap_err().reason.contains("already exists"));

    assert!(!dir.join("a.claktory").exists() && !home.join("moved.json").exists() && home.join("auth.json").exists());

    // A folder left out is said to be, and isn't in the bundle.
    let skipping = Options { out: Some(dir.join("e.claktory")), skip: vec![repo.clone()], ..Options::default() };
    let exported = move_export::export(&place(&dir, 0), &skipping).expect("exported");
    assert!(exported.manifest.folders.is_empty());
    assert_eq!(exported.manifest.skipped[0].why, "left out (--skip)");
}

/// A launchctl that notes what it's asked; the job is never loaded.
fn fake_launchctl(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("launchctl");
    let script = format!("#!/bin/sh\necho \"$@\" >> '{}'\n[ \"$1\" = print ] && exit 113\nexit 0\n", dir.join("calls").display());
    std::fs::write(&path, script).expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

fn calls(dir: &Path) -> Vec<String> {
    let calls = std::fs::read_to_string(dir.join("calls")).unwrap_or_default();
    std::fs::remove_file(dir.join("calls")).ok();
    calls.lines().map(|line| line.split(' ').next().unwrap_or_default().to_owned()).collect()
}

#[tokio::test]
async fn the_service_is_turned_off_for_the_move_and_on_again_when_it_fails_or_is_undone() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir();
    let (factory, _addr, _repo) = claktory(&dir).await;
    drop(factory);
    let home = dir.join("home");
    let service = Service::Launchd {
        launchctl: fake_launchctl(&dir),
        domain: "gui/501".into(),
        label: "dev.factory.factoryd".into(),
        plist: dir.join("job.plist"),
    };
    let place = Place { service, ..place(&dir, 0) };

    // Moved: turned off before it stops, and left off.
    let options = Options { is_final: true, out: Some(dir.join("move.claktory")), ..Options::default() };
    move_export::export(&place, &options).expect("moved");
    assert_eq!(calls(&dir), ["disable", "bootout", "print"]);

    // Undone: on again, and started.
    move_export::restore(&place).expect("restored");
    assert_eq!(calls(&dir), ["enable", "bootstrap", "print"]);

    // A move that fails after Claktory stopped (here, its folder can't take
    // the tombstone): it starts again, and nothing changed.
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o555)).expect("chmod");
    let options = Options { is_final: true, out: Some(dir.join("failed.claktory")), ..Options::default() };
    let failed = move_export::export(&place, &options);
    std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let refused = failed.unwrap_err();
    assert!(refused.reason.to_lowercase().contains("nothing was changed"), "{}", refused.reason);
    assert_eq!(calls(&dir), ["disable", "bootout", "print", "enable", "bootstrap", "print"]);
    assert!(!dir.join("failed.claktory").exists(), "no bundle of a move that didn't happen");
    assert!(home.join("auth.json").exists() && !home.join("moved.json").exists() && !home.join("moved").exists());
}

#[tokio::test]
async fn restore_stops_a_backend_in_moved_mode_so_the_claktory_can_have_its_port() {
    let dir = temp_dir();
    let (factory, _addr, _repo) = claktory(&dir).await;
    drop(factory);
    let home = dir.join("home");
    let place = place(&dir, 0);
    let options = Options { is_final: true, out: Some(dir.join("move.claktory")), ..Options::default() };
    move_export::export(&place, &options).expect("moved");

    // Something started it again anyway: it only says it moved.
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut backend = Command::new(env!("CARGO_BIN_EXE_factoryd"))
        .env("FACTORY_HOME", &home)
        .env("FACTORY_PORT", port.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("factoryd");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !home.join("moved-mode.pid").exists() {
        assert!(std::time::Instant::now() < deadline, "moved mode didn't start");
        assert!(backend.try_wait().expect("wait").is_none(), "it stopped");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    move_export::restore(&place).expect("restored");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while backend.try_wait().expect("wait").is_none() {
        assert!(std::time::Instant::now() < deadline, "moved mode is still running");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(home.join("auth.json").exists() && !home.join("moved.json").exists());
}

/// The `factoryd` a person runs in Terminal is often the backend's own
/// binary (the installed command links to it): that alone isn't running
/// inside Claktory.
#[tokio::test]
async fn a_final_export_runs_from_the_backends_own_binary_in_a_shell() {
    let dir = temp_dir();
    let (factory, _addr, _repo) = claktory(&dir).await;
    drop(factory);
    let home = dir.join("home");
    std::fs::create_dir_all(home.join("bin")).expect("bin");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_factoryd"), home.join("bin/factoryd")).expect("the backend");
    std::os::unix::fs::symlink(home.join("bin/factoryd"), dir.join("factoryd")).expect("the command");
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let out = Command::new(dir.join("factoryd"))
        .args(["export", "--final", "--json", "--out"])
        .arg(dir.join("move.claktory"))
        .env("FACTORY_HOME", &home)
        .env("FACTORY_PORT", port.to_string())
        .env("HOME", &dir)
        .output()
        .expect("factoryd export");
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{said}{}", String::from_utf8_lossy(&out.stderr));
    assert!(home.join("moved.json").exists() && dir.join("move.claktory").exists(), "{said}");
}
