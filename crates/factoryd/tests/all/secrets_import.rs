//! Files the vault imports (secrets_request's `import`): an agent names
//! files on this computer, the person approves on a card that shows which
//! (never their contents), and Claktory reads them into the vault itself,
//! binary files too. Refused: anything but a plain file reached through no
//! link, over 1 MiB, inside Claktory's own folders, or changed since it was
//! asked for. Fugo's hostile paths, each.

use std::path::{Path, PathBuf};

use crate::common::{Client, MODEL, View, apply_request, create_agent, create_project, start, temp_dir};
use crate::pipes::{call, idle, incoming};
use factory_protocol::v1::{self, client_message, entry::Kind, server_message};

/// A project "Site" with Bo; the test's folder as it really is (no link in
/// it, as a path to import must be) and a folder of files to import.
async fn setup() -> (PathBuf, PathBuf, Client, View, std::sync::Arc<factoryd::Factory>) {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Bo", "")).await;
    let files = dir.canonicalize().expect("real").join("files");
    std::fs::create_dir_all(&files).expect("files");
    (dir, files, client, view, factory)
}

fn pending(view: &View) -> v1::SecretRequest {
    view.entries
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) if request.state() == v1::SecretRequestState::Pending => Some(request.clone()),
            _ => None,
        })
        .expect("a request")
}

fn decided(view: &View, id: &str) -> v1::SecretRequest {
    view.entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) if request.id == id => Some(request.clone()),
            _ => None,
        })
        .expect("the request")
}

fn event_text(view: &View, containing: &str) -> Option<String> {
    view.entries.iter().find_map(|entry| match &entry.kind {
        Some(Kind::Event(event)) if event.text.contains(containing) => Some(event.text.clone()),
        _ => None,
    })
}

fn secret(view: &View, name: &str) -> v1::SecretInfo {
    view.workspace.vault.clone().expect("vault").secrets.into_iter().find(|secret| secret.name == name).expect("the secret")
}

fn ask(name: &str, files: &[(&str, &Path)]) -> String {
    let import: Vec<serde_json::Value> =
        files.iter().map(|(variable, path)| serde_json::json!({"variable": variable, "path": path.to_str().expect("text")})).collect();
    serde_json::json!({"name": name, "description": "For signing.", "import": import, "reason": "To sign the release."}).to_string()
}

fn approve(id: &str) -> v1::DecideSecretRequest {
    v1::DecideSecretRequest {
        agent_id: "bo".into(),
        request_id: id.into(),
        approve: true,
        ask_each_time: Some(false),
        ..Default::default()
    }
}

async fn decide(client: &mut Client, view: &mut View, decision: v1::DecideSecretRequest) -> server_message::Body {
    apply_request(client, view, client_message::Body::DecideSecretRequest(decision)).await
}

/// Every file under `dir` holding `value`, but those under `except`.
fn files_holding(dir: &Path, value: &[u8], except: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.starts_with(except) || path.canonicalize().is_ok_and(|real| real.starts_with(except)) {
                continue;
            }
            if path.is_dir() {
                folders.push(path);
            } else if std::fs::read(&path).is_ok_and(|bytes| bytes.windows(value.len()).any(|window| window == value)) {
                found.push(path);
            }
        }
    }
    found
}

/// 48 bytes that aren't text, different each run.
fn binary() -> Vec<u8> {
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_nanos() as u64;
    let mut bytes = vec![0xff, 0x00, 0x80];
    bytes.extend((0..45u64).map(|i| (seed.rotate_left(i as u32 % 64) ^ i.wrapping_mul(0x9e37_79b9)) as u8));
    bytes
}

#[tokio::test]
async fn the_vault_imports_a_text_and_a_binary_file_that_no_one_sees() {
    let (dir, files, mut client, mut view, _factory) = setup().await;
    let p12 = binary();
    let password = format!("storePassword=Zq8{}", std::process::id() * 7919);
    std::fs::write(files.join("upload.p12"), &p12).expect("p12");
    std::fs::write(files.join("key.properties"), format!("{password}\n")).expect("properties");
    let asked = ask("Android upload", &[("UPLOAD_P12", &files.join("upload.p12")), ("KEY_PROPERTIES", &files.join("key.properties"))]);
    let told = call(&mut client, &mut view, "bo", "secrets_request", &asked).await;
    assert!(told.starts_with("Asked to import UPLOAD_P12 ("), "{told}");
    assert!(told.contains("48 bytes") && told.contains("You never see the contents."), "{told}");

    // The card: each file by its path, size, time and keyed fingerprint.
    let request = pending(&view);
    assert_eq!(request.imports.len(), 2);
    let card = &request.imports[0];
    assert_eq!((card.variable.as_str(), card.path.as_str(), card.size), ("UPLOAD_P12", files.join("upload.p12").to_str().unwrap(), 48));
    assert!(card.modified_ms > 0 && card.problem.is_empty() && card.sensitive.is_empty(), "{card:?}");
    let groups: Vec<&str> = card.fingerprint.split(' ').collect();
    assert!(groups.len() == 3 && groups.iter().all(|group| group.len() == 4 && group.chars().all(|c| c.is_ascii_hexdigit())), "{card:?}");
    assert!(!request.asker_read_pipes);
    let variables = request.new_secret.clone().expect("wanted").variables;
    assert!(variables.iter().all(|variable| variable.file), "{variables:?}");
    // Not an admin agent's to find.
    let to_admin = v1::DecideSecretRequest { approve_only: true, ..approve(&request.id) };
    assert!(matches!(decide(&mut client, &mut view, to_admin).await, server_message::Body::Error(_)));

    // Approved, given to Bo.
    let fingerprint = card.fingerprint.clone();
    decide(&mut client, &mut view, v1::DecideSecretRequest { grant_to_asker: true, ..approve(&request.id) }).await;
    view.follow(&mut client, |view| event_text(view, "the vault imported").is_some() && idle(view, "bo")).await;
    let told = event_text(&view, "the vault imported").expect("told");
    assert!(told.contains("the vault imported Android upload and gave it to you"), "{told}");
    assert!(
        told.contains(&format!("UPLOAD_P12: {} (binary file, 48 bytes, fingerprint {fingerprint})", files.join("upload.p12").display())),
        "{told}"
    );
    assert!(told.contains("KEY_PROPERTIES:") && told.contains("(text file, "), "{told}");
    assert!(!told.contains(&password), "{told}");
    assert_eq!(decided(&view, &request.id).state(), v1::SecretRequestState::Granted);
    let kept = secret(&view, "Android upload");
    assert!(kept.description.starts_with("For signing.\n\nImported from this computer:\n"), "{kept:?}");
    assert_eq!(kept.grants.iter().map(|grant| grant.agent_id.as_str()).collect::<Vec<_>>(), ["bo"]);
    assert!(files.join("upload.p12").exists(), "the originals stay unless the person asks");

    // The command's file holds the bytes exactly; output never shows them.
    let same = call(
        &mut client,
        &mut view,
        "bo",
        "bash",
        &format!(
            r#"{{"command":"cmp \"$UPLOAD_P12\" '{}' && echo same","secrets":["Android upload"]}}"#,
            files.join("upload.p12").display()
        ),
    )
    .await;
    assert!(same.contains("same"), "{same}");
    let cat = call(&mut client, &mut view, "bo", "bash", r#"{"command":"cat \"$KEY_PROPERTIES\"","secrets":["Android upload"]}"#).await;
    assert!(cat.contains("[secret Android upload/KEY_PROPERTIES]") && !cat.contains("Zq8"), "{cat}");
    let encoded = call(
        &mut client,
        &mut view,
        "bo",
        "bash",
        r#"{"command":"base64 < \"$UPLOAD_P12\"; xxd -p -c 256 \"$UPLOAD_P12\"","secrets":["Android upload"]}"#,
    )
    .await;
    assert!(encoded.matches("[secret Android upload/UPLOAD_P12]").count() >= 1, "{encoded}");
    let hex: String = p12.iter().map(|byte| format!("{byte:02x}")).collect();
    assert!(!encoded.contains(&hex[..32]), "{encoded}");

    // Its values can't be edited; its name can.
    let edit = v1::UpdateSecret {
        id: kept.id.clone(),
        name: "Android upload".into(),
        description: kept.description.clone(),
        entries: vec![v1::SecretEntry { variable: "UPLOAD_P12".into(), value: "x".into(), file: true }],
    };
    let refused = apply_request(&mut client, &mut view, client_message::Body::UpdateSecret(edit)).await;
    let server_message::Body::Error(error) = refused else { panic!("{refused:?}") };
    assert!(error.message.contains("was imported from a file, so its values can't be edited"), "{error:?}");
    let rename =
        v1::UpdateSecret { id: kept.id.clone(), name: "Upload key".into(), description: kept.description.clone(), entries: Vec::new() };
    assert!(!matches!(
        apply_request(&mut client, &mut view, client_message::Body::UpdateSecret(rename)).await,
        server_message::Body::Error(_)
    ));

    // Its history says what came from where.
    let uses =
        apply_request(&mut client, &mut view, client_message::Body::ListSecretUses(v1::ListSecretUses { id: kept.id.clone(), limit: 0 }))
            .await;
    let server_message::Body::SecretUses(uses) = uses else { panic!("{uses:?}") };
    let import = uses.uses.iter().find(|used| used.kind == "import").expect("an import");
    assert_eq!(import.agent_id, "bo");
    assert!(import.detail.contains(&fingerprint) && import.detail.contains("given to bo; originals kept"), "{import:?}");

    // Nowhere in the clear: not the transcript, the workspace, nor any file
    // Claktory keeps.
    assert!(!format!("{:?}", view.entries).contains(&password));
    assert!(!format!("{:?}", view.workspace).contains(&password));
    assert_eq!(files_holding(&dir, password.as_bytes(), &files), Vec::<PathBuf>::new());
    assert_eq!(files_holding(&dir, &p12, &files), Vec::<PathBuf>::new());
    let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &p12);
    assert_eq!(files_holding(&dir, encoded.as_bytes(), &files), Vec::<PathBuf>::new());
}

#[tokio::test]
async fn an_import_is_refused_for_anything_but_a_plain_file() {
    let (dir, files, mut client, mut view, _factory) = setup().await;
    std::fs::create_dir_all(files.join("real")).expect("real");
    std::fs::write(files.join("real/key.p12"), binary()).expect("file");
    std::os::unix::fs::symlink(files.join("real"), files.join("folder")).expect("link");
    std::os::unix::fs::symlink(files.join("real/key.p12"), files.join("link.p12")).expect("link");
    let fifo = std::ffi::CString::new(files.join("fifo").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    std::fs::write(files.join("big"), vec![b'a'; 1024 * 1024 + 1]).expect("big");
    let vault_db = dir.canonicalize().unwrap().join("home/vault/vault.db");
    assert!(vault_db.exists(), "the test's vault");
    let refusals = [
        (files.join("folder/key.p12"), "is a symbolic link"),
        (files.join("link.p12"), "is a symbolic link"),
        (files.join("fifo"), "isn't a file"),
        (files.join("real"), "isn't a file"),
        (files.join("big"), "over 1 MB"),
        (files.join("missing.p12"), "There's no file at"),
        (vault_db, "inside Claktory's own folder"),
        (PathBuf::from("/dev/zero"), "Claktory doesn't import from /dev"),
        (PathBuf::from("relative/key.p12"), "full path"),
        (files.join("real/../real/key.p12"), "without . or .."),
    ];
    for (path, why) in refusals {
        let told = call(&mut client, &mut view, "bo", "secrets_request", &ask("K", &[("KEY", &path)])).await;
        assert!(told.contains(why), "{}: {told}", path.display());
    }
    // The temporary folder through its link (/var → /private/var on a Mac).
    if dir != dir.canonicalize().unwrap() {
        let told = call(&mut client, &mut view, "bo", "secrets_request", &ask("K", &[("KEY", &dir.join("files/real/key.p12"))])).await;
        assert!(told.contains("is a symbolic link"), "{told}");
    }
    // Another name for the same file (an ssh key, say) would be a path
    // other than the card's.
    std::fs::hard_link(files.join("real/key.p12"), files.join("innocent.p12")).expect("hard link");
    let told = call(&mut client, &mut view, "bo", "secrets_request", &ask("K", &[("KEY", &files.join("innocent.p12"))])).await;
    assert!(told.contains("hard links"), "{told}");
    let generate = r#"{"name":"K","generate":"ssh-ed25519","import":[{"variable":"K","path":"/tmp/x"}],"reason":"x"}"#;
    assert!(call(&mut client, &mut view, "bo", "secrets_request", generate).await.contains("not both"));
    assert!(!view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::SecretRequest(_)))), "none was asked");

    // A name that reads backwards on screen is kept as it is: the app shows
    // the character escaped.
    let tricky = files.join("invoice\u{202E}txt.p12");
    std::fs::write(&tricky, b"not a pdf").expect("file");
    call(&mut client, &mut view, "bo", "secrets_request", &ask("Tricky", &[("KEY", &tricky)])).await;
    assert_eq!(pending(&view).imports[0].path, tricky.to_str().unwrap());
}

#[tokio::test]
async fn a_file_changed_or_replaced_since_it_was_asked_is_refused() {
    let (_dir, files, mut client, mut view, _factory) = setup().await;
    // Edited.
    std::fs::write(files.join("a.p12"), binary()).expect("file");
    call(&mut client, &mut view, "bo", "secrets_request", &ask("A", &[("KEY", &files.join("a.p12"))])).await;
    let edited = pending(&view);
    std::fs::write(files.join("a.p12"), [binary(), vec![1]].concat()).expect("edited");
    // The card says so before the person tries (it's looked at every few
    // seconds), and Import is refused all the same.
    let problem = |view: &View, id: &str| decided(view, id).imports.first().map(|file| file.problem.clone()).unwrap_or_default();
    view.follow(&mut client, |view| problem(view, &edited.id) == "The file changed since it was asked for.").await;
    let refused = decide(&mut client, &mut view, approve(&edited.id)).await;
    let server_message::Body::Error(error) = refused else { panic!("{refused:?}") };
    assert!(error.message.contains("changed since it was asked for"), "{error:?}");
    assert!(view.workspace.vault.clone().expect("vault").secrets.is_empty(), "nothing was imported");
    // Deleted.
    std::fs::write(files.join("gone.p12"), binary()).expect("file");
    call(&mut client, &mut view, "bo", "secrets_request", &ask("Gone", &[("KEY", &files.join("gone.p12"))])).await;
    let gone = pending(&view);
    assert_eq!(problem(&view, &gone.id), "");
    std::fs::remove_file(files.join("gone.p12")).expect("deleted");
    view.follow(&mut client, |view| problem(view, &gone.id) == "The file isn't there any more.").await;

    // Replaced by another file with the same bytes and time.
    let bytes = binary();
    std::fs::write(files.join("b.p12"), &bytes).expect("file");
    call(&mut client, &mut view, "bo", "secrets_request", &ask("B", &[("KEY", &files.join("b.p12"))])).await;
    let swapped = pending(&view);
    let modified = std::fs::metadata(files.join("b.p12")).unwrap().modified().unwrap();
    std::fs::write(files.join("b2"), &bytes).expect("file");
    std::fs::File::options().write(true).open(files.join("b2")).unwrap().set_modified(modified).unwrap();
    std::fs::rename(files.join("b2"), files.join("b.p12")).expect("swapped");
    let refused = decide(&mut client, &mut view, approve(&swapped.id)).await;
    let server_message::Body::Error(error) = refused else { panic!("{refused:?}") };
    assert!(error.message.contains("changed since it was asked for"), "{error:?}");
    // Given another name since.
    std::fs::write(files.join("d.p12"), binary()).expect("file");
    call(&mut client, &mut view, "bo", "secrets_request", &ask("D", &[("KEY", &files.join("d.p12"))])).await;
    let named = pending(&view);
    std::fs::hard_link(files.join("d.p12"), files.join("d-too.p12")).expect("hard link");
    let refused = decide(&mut client, &mut view, approve(&named.id)).await;
    let server_message::Body::Error(error) = refused else { panic!("{refused:?}") };
    assert!(error.message.contains("hard links"), "{error:?}");
    // Through a link put there since.
    std::fs::write(files.join("c.p12"), binary()).expect("file");
    call(&mut client, &mut view, "bo", "secrets_request", &ask("C", &[("KEY", &files.join("c.p12"))])).await;
    let linked = pending(&view);
    std::fs::rename(files.join("c.p12"), files.join("c-real.p12")).unwrap();
    std::os::unix::fs::symlink(files.join("c-real.p12"), files.join("c.p12")).unwrap();
    let refused = decide(&mut client, &mut view, approve(&linked.id)).await;
    let server_message::Body::Error(error) = refused else { panic!("{refused:?}") };
    assert!(error.message.contains("is a symbolic link"), "{error:?}");
}

/// A file of exactly 1 MiB that isn't text is within the vault's limit,
/// and taking it in (and every vault change after it) answers at once:
/// hiding its forms once took the whole backend tens of seconds and GBs.
#[tokio::test]
async fn a_whole_mib_imports_and_the_vault_answers_at_once_after_it() {
    let (_dir, files, mut client, mut view, _factory) = setup().await;
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_nanos() as u64 | 1;
    let noise: Vec<u8> = (0..1024 * 1024)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 24) as u8
        })
        .collect();
    std::fs::write(files.join("exact.bin"), &noise).expect("file");
    std::fs::write(files.join("small.txt"), "hello").expect("file");
    for (name, file) in [("Exact", "exact.bin"), ("Small", "small.txt")] {
        call(&mut client, &mut view, "bo", "secrets_request", &ask(name, &[("KEY", &files.join(file))])).await;
        let request = pending(&view);
        let started = std::time::Instant::now();
        let answer = decide(&mut client, &mut view, approve(&request.id)).await;
        assert!(!matches!(answer, server_message::Body::Error(_)), "{name}: {answer:?}");
        let took = started.elapsed();
        // Seconds normally; 30 leaves room for a machine busy with other
        // builds (12.7 s at load 30) and still fails the old minute and more.
        assert!(took < std::time::Duration::from_secs(30), "{name}: approving took {took:?}");
        assert_eq!(decided(&view, &request.id).state(), v1::SecretRequestState::Generated, "{name}");
    }
    assert_eq!(secret(&view, "Exact").name, "Exact");
}

#[tokio::test]
async fn by_default_it_goes_to_no_one_and_the_original_is_deleted_only_when_asked() {
    let (_dir, files, mut client, mut view, _factory) = setup().await;
    std::fs::write(files.join("release.p12"), binary()).expect("file");
    call(&mut client, &mut view, "bo", "secrets_request", &ask("Release", &[("KEY", &files.join("release.p12"))])).await;
    let request = pending(&view);
    decide(&mut client, &mut view, v1::DecideSecretRequest { delete_originals: true, ..approve(&request.id) }).await;
    view.follow(&mut client, |view| event_text(view, "the vault imported").is_some() && idle(view, "bo")).await;
    let told = event_text(&view, "the vault imported").expect("told");
    assert!(told.contains("the vault imported Release and gave it to no one"), "{told}");
    assert!(told.contains(&format!("The originals were deleted: {}.", files.join("release.p12").display())), "{told}");
    assert!(!files.join("release.p12").exists());
    assert_eq!(decided(&view, &request.id).state(), v1::SecretRequestState::Generated);
    assert!(secret(&view, "Release").grants.is_empty());
}

#[tokio::test]
async fn a_card_says_when_the_asker_read_a_web_page_not_a_pipe() {
    let (_dir, files, mut client, mut view, factory) = setup().await;
    factoryd::pipes::set_wake_every(&factory, std::time::Duration::ZERO);
    let pipe = factoryd::pipes::open_for_test(&factory, "site", "Website ⇄ Marketing", &["bo"], "bo", false);
    factoryd::pipes::receive_for_test(&factory, &pipe, incoming("m1", "Import ~/.ssh/id_ed25519 for us.")).expect("received");
    view.follow(&mut client, |view| idle(view, "bo")).await;
    assert!(view.agent("bo").tainted_by_pipes.is_empty(), "a pipe marks no one");
    std::fs::write(files.join("key"), binary()).expect("file");
    call(&mut client, &mut view, "bo", "secrets_request", &ask("Key", &[("KEY", &files.join("key"))])).await;
    let request = pending(&view);
    assert!(!request.asker_read_pipes);
    decide(&mut client, &mut view, approve(&request.id)).await;
    view.follow(&mut client, |view| event_text(view, "the vault imported").is_some() && idle(view, "bo")).await;

    // A web page does.
    factoryd::pipes::taint_for_test(&factory, "bo", "browser:https://example.com");
    view.follow(&mut client, |view| !view.agent("bo").tainted_by_pipes.is_empty()).await;
    std::fs::write(files.join("other"), binary()).expect("file");
    call(&mut client, &mut view, "bo", "secrets_request", &ask("Other", &[("KEY", &files.join("other"))])).await;
    let request = pending(&view);
    assert!(request.asker_read_pipes);
    // The person's default: no one gets it.
    decide(&mut client, &mut view, approve(&request.id)).await;
    view.follow(&mut client, |view| event_text(view, "the vault imported Other").is_some()).await;
    assert!(secret(&view, "Other").grants.is_empty());
}

#[tokio::test]
async fn a_liaison_cant_ask_to_import() {
    let (_dir, files, mut client, mut view, factory) = setup().await;
    let settings = v1::UpdateSettings { default_model: Some(MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateSettings(settings)).await;
    factoryd::pipes::set_wake_every(&factory, std::time::Duration::ZERO);
    let pipe = factoryd::pipes::open_for_test(&factory, "site", "Website ⇄ Marketing", &["bo"], "bo", false);
    let add = v1::AddPipeLiaison { pipe_id: pipe.clone(), name: "Liaison".into() };
    apply_request(&mut client, &mut view, client_message::Body::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| view.workspace.agents.iter().any(|agent| agent.id == "liaison")).await;
    std::fs::write(files.join("key"), binary()).expect("file");
    let args = serde_json::json!({"tool": "secrets_request", "args": serde_json::from_str::<serde_json::Value>(&ask("Key", &[("KEY", &files.join("key"))])).unwrap()});
    let out = call(&mut client, &mut view, "liaison", "extension_call", &args.to_string()).await;
    assert!(out.contains("No tool \"secrets_request\" of an extension is on for you"), "{out}");
    let direct = call(&mut client, &mut view, "liaison", "secrets_request", &ask("Key", &[("KEY", &files.join("key"))])).await;
    assert!(direct.contains("not found"), "{direct}");
    assert!(!view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::SecretRequest(_)))));
}
