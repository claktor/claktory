//! Moving a Claktory to another computer, the import's side (#217): a
//! bundle made from a running Claktory becomes a Claktory here, with its
//! paths moved, its folders and checkouts made again, a new key for its
//! vault, its old computer's app a device, and each agent told. An import
//! that stops never half-starts, carries on when run again, and can be
//! undone.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::common::{Client, View, apply_request, create_agent, create_project, home, start, temp_dir};
use factory_protocol::v1;
use factoryd::home::Home;
use factoryd::move_import::{self, ImportError, Request};
use rusqlite::Connection;

pub(crate) fn run(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A port nothing listens on.
pub(crate) fn free_addr() -> std::net::SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("addr")
}

/// What a source Claktory looks like on its computer: its home, its user's
/// home, the project's repository and the agent's branch.
pub(crate) struct Source {
    pub dir: PathBuf,
    pub home: Home,
    pub user: PathBuf,
    pub repo: PathBuf,
    pub token: String,
}

pub(crate) async fn source() -> Source {
    let dir = temp_dir();
    let user = dir.join("user");
    let repo = user.join("site");
    std::fs::create_dir_all(&repo).expect("repo");
    run(&repo, &["init", "-q", "-b", "main"]);
    run(&repo, &["config", "user.email", "t@example.test"]);
    run(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("README.md"), "one\n").expect("write");
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-q", "-m", "first"]);
    // The agent's branch, with work of its own; its home is "home", so its
    // branches are home/<agent>.
    run(&repo, &["branch", "home/writer"]);
    run(&repo, &["checkout", "-q", "home/writer"]);
    std::fs::write(repo.join("agent.txt"), "agent's work\n").expect("write");
    run(&repo, &["add", "."]);
    run(&repo, &["commit", "-q", "-m", "agent"]);
    run(&repo, &["checkout", "-q", "main"]);

    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &repo)).await;
    // A second project in the workspace: Claktory's own territory, kept
    // relative in the settings, so the move needs no map for it.
    apply_request(&mut client, &mut view, create_project("Notes", &dir.join("home/workspace/notes"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    let home = home(&dir);
    // A monitor with paths in it, and a task that was running.
    let db = Connection::open(home.db_path()).expect("db");
    let spec = serde_json::json!({"check": format!("test -f {}/done", repo.display()), "every": "5m"}).to_string();
    db.execute(
        "INSERT INTO monitors (agent, id, name, prompt, kind, spec, idle_only, created_by, created, state, reason, fires, carry, cursor, updated) VALUES ('writer', 1, 'done', 'Look', 'check', ?1, 1, 'writer', 0, 'active', '', 0, '', '{\"pid\": 4242, \"started\": 1}', 0)",
        [spec],
    )
    .expect("monitor");
    db.execute(
        "INSERT INTO tasks (agent, task, command, pid, log, started) VALUES ('writer', 'task-1', 'cargo build && xcodebuild -version', 99999, ?1, 0)",
        [home.agent_dir("writer").join("tasks/task-1.log").display().to_string()],
    )
    .expect("task");
    db.execute("INSERT INTO messages (agent, source, sender, sender_id, summary, text, created, delivered) VALUES ('writer', 'person', '', '', '', 'Hi', 0, 0)", [])
        .expect("a message");
    let pairing = std::fs::read_to_string(home.root().join("remote/local-pairing")).expect("local pairing");
    let token = pairing.split(['?', '&']).find_map(|part| part.strip_prefix("t=")).expect("a token").to_owned();
    Source { dir, home, user, repo, token }
}

/// Every file under `from`, as bundle paths under `prefix`, but the
/// records' WAL; databases as snapshots.
fn add_tree(writer: &mut factoryd::move_bundle::Writer<std::fs::File>, from: &Path, prefix: &str, scratch: &Path) {
    let mut entries: Vec<_> = std::fs::read_dir(from).expect("read").filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}/{name}");
        let kind = entry.file_type().expect("type");
        if kind.is_dir() {
            writer.dir(&path, 0o700).expect("dir");
            add_tree(writer, &entry.path(), &path, scratch);
        } else if kind.is_file() {
            if name.ends_with("-wal") || name.ends_with("-shm") || name.ends_with(".pid") || name.starts_with("factoryd.log") {
                continue;
            }
            let mut source = entry.path();
            if std::fs::read(&source).is_ok_and(|bytes| bytes.starts_with(b"SQLite format 3\0")) {
                let snapshot = scratch.join(format!("snapshot-{}", path.replace('/', "_")));
                let _ = std::fs::remove_file(&snapshot);
                Connection::open(&source).expect("db").execute("VACUUM INTO ?1", [snapshot.display().to_string()]).expect("snapshot");
                source = snapshot;
            }
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&source).expect("meta").permissions().mode() & 0o777;
            let bytes = std::fs::read(&source).expect("read");
            writer.file(&path, mode, 0, bytes.len() as u64, bytes.as_slice()).expect("file");
        }
    }
}

/// The bundle export would make of `source`, written to a file; and its passphrase.
pub(crate) fn bundle(source: &Source, version: &str) -> (PathBuf, String) {
    bundle_with(source, version, |_| {})
}

/// The same, with the manifest changed first.
fn bundle_with(source: &Source, version: &str, edit: impl FnOnce(&mut serde_json::Value)) -> (PathBuf, String) {
    let scratch = source.dir.join("scratch");
    std::fs::create_dir_all(&scratch).expect("scratch");
    let repo_bundle = scratch.join("repo.bundle");
    let _ = std::fs::remove_file(&repo_bundle);
    run(&source.repo, &["bundle", "create", "-q", &repo_bundle.display().to_string(), "--all"]);
    // The main checkout's uncommitted change, and a file git doesn't track.
    std::fs::write(source.repo.join("README.md"), "one\ntwo\n").expect("write");
    let patch = run(&source.repo, &["diff", "--binary", "HEAD"]) + "\n";
    let head = run(&source.repo, &["rev-parse", "HEAD"]);
    let mut manifest = serde_json::json!({
        "format": 1,
        "export_id": "0123456789abcdef0123456789abcdef",
        "created_ms": 1_763_735_000_000_i64,
        "final": true,
        "version": version,
        "epoch": 3,
        "source": {"machine": "Old Mac", "os": "macos", "arch": "aarch64", "user": "matt",
                   "user_home": source.user, "home": source.home.root(), "dev": false},
        "files": 0, "bytes": 0,
        "requires": {"disk_bytes": 1, "memory_bytes": 1},
        "folders": [{
            "path": source.repo, "kind": "git", "projects": ["site"], "branch": "main", "head": head,
            "remotes": [{"name": "origin", "url": "git@example.test:site.git"}],
            "bundle": "folders/0/repo.bundle", "patch": "folders/0/changes.patch",
            "untracked": "folders/0/untracked", "files": null, "bytes": 1
        }],
        "skipped": [{"path": "/Volumes/media", "why": "left out (--skip)"}],
        "checkouts": [{
            "agent": "writer", "name": "site", "tree": source.home.root().join("trees/writer/site"),
            "repo": source.repo, "branch": "home/writer", "head": head,
            "patch": "checkouts/writer/site/changes.patch", "untracked": null
        }],
        "tasks": [{"agent": "writer", "task": "task-1", "command": "cargo build && xcodebuild -version", "started_ms": 1_763_734_000_000_i64}]
    });
    edit(&mut manifest);
    let (text, key) = factoryd::move_bundle::new_passphrase();
    let path = source.dir.join("site.claktory");
    let file = std::fs::File::create(&path).expect("create");
    let mut writer = factoryd::move_bundle::Writer::new(file, &key).expect("writer");
    writer.bytes("manifest.json", 0o644, manifest.to_string().as_bytes()).expect("manifest");
    writer.dir("home", 0o700).expect("home");
    add_tree(&mut writer, source.home.root(), "home", &scratch);
    let repo_bytes = std::fs::read(&repo_bundle).expect("bundle");
    writer.file("folders/0/repo.bundle", 0o644, 0, repo_bytes.len() as u64, repo_bytes.as_slice()).expect("repo");
    writer.bytes("folders/0/changes.patch", 0o644, patch.as_bytes()).expect("patch");
    writer.bytes("folders/0/untracked/notes/todo.txt", 0o644, b"untracked\n").expect("untracked");
    // The agent's checkout had an uncommitted change on its branch.
    let checkout_patch = "diff --git a/agent.txt b/agent.txt\n--- a/agent.txt\n+++ b/agent.txt\n@@ -1 +1,2 @@\n agent's work\n+more\n";
    writer.bytes("checkouts/writer/site/changes.patch", 0o644, checkout_patch.as_bytes()).expect("checkout patch");
    writer.finish().expect("finish");
    std::fs::write(source.repo.join("README.md"), "one\n").expect("write");
    (path, text)
}

fn request(bundle: &Path, passphrase: &str, from: &Path, to: &Path) -> Request {
    Request {
        bundle: bundle.to_path_buf(),
        passphrase: Some(passphrase.to_owned()),
        map: vec![(from.to_path_buf(), to.to_path_buf())],
        same_identity: false,
    }
}

fn refused(result: Result<move_import::Imported, ImportError>) -> String {
    match result {
        Err(ImportError::Refused(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_claktory_moves_with_its_paths_folders_checkouts_vault_and_devices() {
    let source = source().await;
    // The settings in the form every Claktory reads: absolute paths for
    // everything, the workspace project included.
    let text = std::fs::read_to_string(source.home.config_path()).expect("config");
    let notes = source.home.root().join("workspace/notes");
    assert!(text.contains(&format!("\"path\": \"{}\"", notes.display())), "{text}");
    let (path, passphrase) = bundle(&source, factoryd::update::VERSION);
    let target = temp_dir();
    let (here, user) = (Home::new(target.join("home")), target.join("user"));

    // A newer Claktory's bundle, or a wrong passphrase, is refused before anything changes.
    let wrong = request(&path, "AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA-AAAA", &source.user, &user);
    assert!(refused(move_import::import(&here, free_addr(), &wrong)).contains("passphrase"));
    assert!(move_import::unfinished(&here).is_none() && !here.root().join("import").exists(), "a refusal leaves no trace");
    let imported = move_import::import(&here, free_addr(), &request(&path, &passphrase, &source.user, &user)).expect("imported");
    assert!(move_import::unfinished(&here).is_none(), "it starts as usual");
    assert!(!here.root().join("import").exists());
    assert!(imported.text.contains("Not moved: /Volumes/media"), "{}", imported.text);

    // Settings and records point here.
    let config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(here.config_path()).expect("config")).expect("json");
    let site = user.join("site");
    assert_eq!(config["projects"][0]["path"], site.display().to_string());
    // The old machine's absolute workspace path came out as territory:
    // relative, under this home, with no map pair naming it.
    assert_eq!(config["projects"][1]["path"], "workspace/notes");
    assert!(here.root().join("workspace/notes").is_dir());
    let db = Connection::open(here.db_path()).expect("db");
    let spec: String = db.query_row("SELECT spec FROM monitors WHERE agent = 'writer'", [], |row| row.get(0)).expect("monitor");
    assert!(spec.contains(&format!("test -f {}/done", site.display())), "{spec}");
    let cursor: String = db.query_row("SELECT cursor FROM monitors WHERE agent = 'writer'", [], |row| row.get(0)).expect("cursor");
    assert!(!cursor.contains("4242"), "the old computer's process isn't this one's: {cursor}");
    let (log, status): (String, String) =
        db.query_row("SELECT log, status FROM tasks WHERE task = 'task-1'", [], |row| Ok((row.get(0)?, row.get(1)?))).expect("task");
    assert!(log.starts_with(&here.root().display().to_string()) && status == "stopped by the move", "{log} {status}");
    assert_eq!(std::fs::read_to_string(here.root().join("move-epoch")).expect("epoch").trim(), "3");

    // The folder, with its uncommitted change, untracked files and remote.
    assert_eq!(run(&site, &["branch", "--show-current"]), "main");
    assert_eq!(std::fs::read_to_string(site.join("README.md")).expect("readme"), "one\ntwo\n");
    assert_eq!(std::fs::read_to_string(site.join("notes/todo.txt")).expect("untracked"), "untracked\n");
    assert_eq!(run(&site, &["remote", "get-url", "origin"]), "git@example.test:site.git");
    // The agent's checkout, on its branch with its change.
    let tree = here.root().join("trees/writer/site");
    assert_eq!(run(&tree, &["branch", "--show-current"]), "home/writer");
    assert_eq!(std::fs::read_to_string(tree.join("agent.txt")).expect("agent"), "agent's work\nmore\n");

    // A new key on disk opens the vault; the old one doesn't match it.
    let old_key = std::fs::read_to_string(source.home.root().join("vault/auto.key")).expect("old key");
    let new_key = std::fs::read_to_string(here.root().join("vault/auto.key")).expect("new key");
    assert_ne!(old_key, new_key);

    // The old computer's app is a device now, with its token, and a code to give it.
    let devices: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(here.root().join("remote/devices.json")).expect("devices")).expect("json");
    let devices = devices.as_array().expect("list");
    assert!(devices.iter().all(|device| device.get("local").is_none()), "{devices:?}");
    assert!(!here.root().join("remote/local-pairing").exists(), "made again when it starts");
    let code = imported.old_app_code.expect("a code for the old app");
    assert!(code.contains(&format!("t={}", source.token)), "{code}");

    // Each agent is told, once.
    let (text, summary): (String, String) = db
        .query_row("SELECT text, summary FROM messages WHERE agent = 'writer' AND sender = 'Claktory' AND delivered IS NULL", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("told");
    assert!(summary.starts_with("Claktory moved to "), "{summary}");
    for part in [
        "Claktory moved from Old Mac (macOS arm64) to ",
        &format!("{} → {}", source.user.display(), user.display()),
        &format!("Your checkout is at {}, on home/writer, with your uncommitted changes.", tree.display()),
        "Stopped by the move: task-1 (cargo build && xcodebuild -version)",
    ] {
        assert!(text.contains(part), "{part:?} in {text}");
    }
    if !Path::new("/usr/bin/xcodebuild").exists() {
        assert!(text.contains("Missing on this computer: xcodebuild"), "{text}");
    }
    drop(db);

    // It's a Claktory: it starts, with the project where it is now.
    let (addr, _factory) = start(&target).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.projects[0].path, site.display().to_string());
    assert!(workspace.agents.iter().any(|agent| agent.id == "writer"));

    // Once only: this Claktory has work in it now.
    let again = refused(move_import::import(&here, free_addr(), &request(&path, &passphrase, &source.user, &user)));
    assert!(again.contains("already has work in it"), "{again}");
}

#[tokio::test]
async fn an_import_that_stops_never_half_starts_carries_on_and_can_be_undone() {
    use std::os::unix::fs::PermissionsExt;
    let source = source().await;
    let (path, passphrase) = bundle(&source, factoryd::update::VERSION);
    let target = temp_dir();
    let (here, user) = (Home::new(target.join("home")), target.join("user"));
    // A fresh Claktory was here; its settings are put aside, never lost.
    std::fs::create_dir_all(here.root()).expect("home");
    std::fs::write(here.config_path(), "{\"name\": \"Fresh\"}").expect("fresh");
    // Where the folder goes can't be written: it stops after the home switched over.
    std::fs::create_dir_all(&user).expect("user");
    std::fs::set_permissions(&user, std::fs::Permissions::from_mode(0o500)).expect("chmod");
    let stopped = move_import::import(&here, free_addr(), &request(&path, &passphrase, &source.user, &user));
    assert!(matches!(stopped, Err(ImportError::Failed(_))), "{stopped:?}");
    let why = move_import::unfinished(&here).expect("it won't start");
    assert!(why.contains("hasn't finished"), "{why}");
    assert!(here.root().join("vault/vault.db").exists(), "switched over");

    // Undone: the fresh Claktory's settings are back.
    move_import::abandon(&here).expect("abandoned");
    assert_eq!(std::fs::read_to_string(here.config_path()).expect("config"), "{\"name\": \"Fresh\"}");
    assert!(!here.root().join("vault").exists() && move_import::unfinished(&here).is_none());

    // Stopped again, then carried on: the folder can be written now.
    let stopped = move_import::import(&here, free_addr(), &request(&path, &passphrase, &source.user, &user));
    assert!(stopped.is_err());
    std::fs::set_permissions(&user, std::fs::Permissions::from_mode(0o700)).expect("chmod");
    // The passphrase isn't needed again: the bundle is unpacked.
    assert!(!move_import::needs_passphrase(&here, &path));
    let resumed = Request { bundle: path.clone(), passphrase: None, map: Vec::new(), same_identity: false };
    move_import::import(&here, free_addr(), &resumed).expect("finished");
    assert!(move_import::unfinished(&here).is_none());
    assert_eq!(std::fs::read_to_string(user.join("site/README.md")).expect("readme"), "one\ntwo\n");
    let config = std::fs::read_to_string(here.config_path()).expect("config");
    assert!(config.contains(&user.join("site").display().to_string()), "{config}");
    // The fresh Claktory's settings went to the trash.
    let trash: Vec<_> = std::fs::read_dir(here.root().join("trash")).expect("trash").filter_map(Result::ok).collect();
    assert_eq!(std::fs::read_to_string(trash[0].path().join("config.json")).expect("kept"), "{\"name\": \"Fresh\"}");
}

#[tokio::test]
async fn an_import_is_refused_where_it_cant_go() {
    let source = source().await;
    let target = temp_dir();
    let (here, user) = (Home::new(target.join("home")), target.join("user"));
    // Over a folder that's there already.
    let (path, passphrase) = bundle(&source, factoryd::update::VERSION);
    std::fs::create_dir_all(user.join("site")).expect("site");
    std::fs::write(user.join("site/mine.txt"), "mine").expect("write");
    let why = refused(move_import::import(&here, free_addr(), &request(&path, &passphrase, &source.user, &user)));
    assert!(why.contains("already there and isn't empty"), "{why}");
    assert_eq!(std::fs::read_to_string(user.join("site/mine.txt")).expect("kept"), "mine");
    assert!(move_import::unfinished(&here).is_none(), "nothing changed, so Claktory starts as it was");
    // While a Claktory runs here.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let why =
        refused(move_import::import(&here, listener.local_addr().expect("addr"), &request(&path, &passphrase, &source.user, &target)));
    assert!(why.contains("Claktory is running here"), "{why}");
    assert!(move_import::unfinished(&here).is_none());
    // Over a Claktory with work in it: the source's own home, say.
    let why = refused(move_import::import(&source.home, free_addr(), &request(&path, &passphrase, &source.user, &target)));
    assert!(why.contains("already has work in it"), "{why}");
    assert!(move_import::unfinished(&source.home).is_none());
    // Onto a disk without room for it: said before anything is unpacked.
    let (big, passphrase) =
        bundle_with(&source, factoryd::update::VERSION, |manifest| manifest["requires"]["disk_bytes"] = (1_u64 << 60).into());
    let why = refused(move_import::import(&here, free_addr(), &request(&big, &passphrase, &source.user, &target)));
    assert!(why.starts_with("There isn't room: this needs ") && why.ends_with("Make room, then try again."), "{why}");
    assert!(!here.root().join("import").exists(), "nothing was unpacked");
}

/// A client of a Claktory in moved mode: says hello with `token`.
struct MovedClient(tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, u64);

impl MovedClient {
    async fn open(addr: std::net::SocketAddr) -> MovedClient {
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}")).await.expect("connect");
        MovedClient(socket, 6)
    }

    async fn connect(addr: std::net::SocketAddr, token: &str) -> (MovedClient, v1::Error) {
        let mut client = MovedClient::open(addr).await;
        let hello = v1::Hello { token: token.to_owned(), ..Default::default() };
        let answer = client.ask(v1::client_message::Body::Hello(hello)).await.expect("an answer");
        let Some(v1::server_message::Body::Error(error)) = answer else { panic!("an error: {answer:?}") };
        (client, error)
    }

    /// The answer, or None when the connection closed instead.
    async fn ask(&mut self, body: v1::client_message::Body) -> Option<Option<v1::server_message::Body>> {
        use futures_util::{SinkExt, StreamExt};
        use prost::Message as _;
        self.1 += 1;
        let message = v1::ClientMessage { id: self.1, body: Some(body) };
        self.0.send(tokio_tungstenite::tungstenite::Message::Binary(message.encode_to_vec().into())).await.ok()?;
        let Some(Ok(tokio_tungstenite::tungstenite::Message::Binary(bytes))) = self.0.next().await else { return None };
        let answer = v1::ServerMessage::decode(bytes).expect("decode");
        assert_eq!(answer.reply_to, self.1);
        Some(answer.body)
    }

    async fn delete(&mut self, typed: &str) -> v1::server_message::Body {
        let request = v1::DeleteMovedCopy { typed_word: typed.to_owned() };
        self.ask(v1::client_message::Body::DeleteMovedCopy(request)).await.expect("an answer").expect("a body")
    }
}

fn refusal(body: v1::server_message::Body) -> (v1::ErrorCode, String) {
    let v1::server_message::Body::Error(error) = body else { panic!("a refusal: {body:?}") };
    (error.code(), error.message)
}

#[tokio::test]
async fn a_claktory_that_moved_away_runs_nothing_says_where_and_its_app_can_delete_the_copy() {
    let dir = temp_dir();
    let here = Home::new(dir.join("home"));
    std::fs::create_dir_all(here.root().join("moved/remote")).expect("home");
    std::fs::write(
        here.root().join("moved.json"),
        r#"{"format":1,"export_id":"ab","at_ms":1763735100000,"epoch":1,"machine":"Old Mac","to":"cloud-box","to_os":"linux","version":"v0.1.14","bundle":"/x"}"#,
    )
    .expect("tombstone");
    // The copy left behind: the fenced pairing code, its records, and the
    // workspace, which stays.
    std::fs::write(here.root().join("moved/remote/local-pairing"), "factory://?t=0123abcd&a=127.0.0.1%3A7420\n").expect("code");
    std::fs::create_dir_all(here.root().join("agents/ed")).expect("agents");
    std::fs::create_dir_all(here.root().join("workspace/notes")).expect("workspace");
    std::fs::write(here.root().join("workspace/notes/a.md"), "kept").expect("note");
    let moved = move_import::moved::read(&here).expect("moved");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let serving = here.clone();
    tokio::spawn(async move { move_import::moved::serve(listener, &serving, moved).await });

    // Any client is told it moved, and where.
    let (mut stranger, error) = MovedClient::connect(addr, "").await;
    assert_eq!(error.code(), v1::ErrorCode::Moved);
    assert!(error.message.starts_with("This Claktory moved to cloud-box on "), "{}", error.message);
    let away = error.moved.expect("where it went");
    assert_eq!((away.to.as_str(), away.to_os.as_str(), away.machine.as_str()), ("cloud-box", "linux", "Old Mac"));
    assert_eq!((away.at_ms, away.deleted_ms), (1763735100000, 0));
    // Nothing of the Claktory was made.
    assert!(!here.db_path().exists() && !here.root().join("remote").exists());
    // Only this computer's own app may delete the copy: with the code's
    // token. Anyone else is told once, and let go.
    assert!(
        stranger.ask(v1::client_message::Body::DeleteMovedCopy(v1::DeleteMovedCopy::default())).await.is_none_or(|answer| answer.is_none())
    );
    assert_eq!(refusal(MovedClient::open(addr).await.delete("delete").await).0, v1::ErrorCode::Unauthorized);
    let (mut wrong, error) = MovedClient::connect(addr, "ffff").await;
    assert_eq!(error.code(), v1::ErrorCode::Moved);
    assert!(
        wrong.ask(v1::client_message::Body::DeleteMovedCopy(v1::DeleteMovedCopy::default())).await.is_none_or(|answer| answer.is_none())
    );
    let (mut own, _) = MovedClient::connect(addr, "0123abcd").await;
    // Anything else it asks is answered that it moved.
    let other = own.ask(v1::client_message::Body::SelectAgent(v1::SelectAgent::default())).await.expect("an answer").expect("a body");
    assert_eq!(refusal(other).0, v1::ErrorCode::Moved);
    let (code, why) = refusal(own.delete("yes").await);
    assert_eq!((code, why.as_str()), (v1::ErrorCode::InvalidRequest, "Type delete to confirm."));
    assert!(here.root().join("agents/ed").exists());
    assert!(matches!(own.delete("delete").await, v1::server_message::Body::Ack(_)));
    // Deleted, all but the workspace and moved.json; the connection closes.
    assert!(own.ask(v1::client_message::Body::SelectAgent(v1::SelectAgent::default())).await.is_none_or(|answer| answer.is_none()));
    assert!(!here.root().join("agents").exists() && !here.root().join("moved").exists());
    assert_eq!(std::fs::read_to_string(here.root().join("workspace/notes/a.md")).expect("kept"), "kept");
    // It still runs, saying so, and moving back can still stop it.
    assert!(here.root().join("moved-mode.pid").exists());
    let (mut again, error) = MovedClient::connect(addr, "0123abcd").await;
    assert!(error.moved.expect("where").deleted_ms > 0);
    // The code went with the copy: nobody can ask again.
    assert!(
        again.ask(v1::client_message::Body::DeleteMovedCopy(v1::DeleteMovedCopy::default())).await.is_none_or(|answer| answer.is_none())
    );

    // A Claktory that hasn't moved has no copy to delete.
    let (addr, _factory) = start(&temp_dir()).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let answer = client.request(v1::client_message::Body::DeleteMovedCopy(v1::DeleteMovedCopy { typed_word: "delete".into() })).await;
    assert!(
        matches!(&answer.body, v1::server_message::Body::Error(error) if error.code() == v1::ErrorCode::InvalidRequest),
        "{:?}",
        answer.body
    );
}

/// What `factoryd export` really writes, imported: the two halves agree.
#[tokio::test]
async fn a_bundle_export_made_imports() {
    use factoryd::move_export::{Options, Place, Service};
    let source = source().await;
    std::fs::write(source.repo.join("README.md"), "one\nchanged\n").expect("write");
    let out = source.dir.join("out.claktory");
    let place = Place { home: source.home.clone(), port: 0, user_home: source.user.clone(), dev: false, service: Service::None };
    let exported = factoryd::move_export::export(&place, &Options { out: Some(out.clone()), ..Options::default() }).expect("exported");
    let target = temp_dir();
    let (here, user) = (Home::new(target.join("home")), target.join("user"));
    // Made without --final, its Claktory still runs: a second one with its
    // identity is made only when asked.
    let mut asked = request(&out, &exported.passphrase, &source.user, &user);
    let why = refused(move_import::import(&here, free_addr(), &asked));
    assert!(why.contains("without --final") && why.contains("the same tunnel key") && why.contains("--same-identity"), "{why}");
    assert!(!here.root().join("import").exists());
    asked.same_identity = true;
    let imported = move_import::import(&here, free_addr(), &asked).expect("imported");
    assert!(imported.old_app_code.is_some(), "{}", imported.text);
    assert!(imported.text.contains("Run this Claktory in one place only"), "{}", imported.text);
    let site = user.join("site");
    assert_eq!(std::fs::read_to_string(site.join("README.md")).expect("readme"), "one\nchanged\n");
    assert!(run(&site, &["branch", "--list", "home/writer"]).contains("home/writer"), "the agent's branch came along");
    let config = std::fs::read_to_string(here.config_path()).expect("config");
    assert!(config.contains(&site.display().to_string()), "{config}");
    let (_addr, _factory) = start(&target).await;
}
