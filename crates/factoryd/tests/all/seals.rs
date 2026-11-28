//! Seals: an agent with the Seal extension records a file's version with a
//! note; the record is Claktory's and is never changed or removed, a
//! viewer reading the file hears whether it still matches, the version
//! sealed can be opened after the file changed, and History shows who
//! sealed what.

use serde_json::json;

use crate::common::{Client, View, apply_request, create_agent, create_project, home_of, start, temp_dir};
use crate::pipes::call;
use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind, server_message::Body};

fn read(path: &std::path::Path, sealed: &str) -> Request {
    Request::ReadFile(v1::ReadFile { path: path.display().to_string(), sealed_sha256: sealed.to_owned(), ..Default::default() })
}

async fn content(client: &mut Client, view: &mut View, request: Request) -> v1::FileContent {
    let Body::FileContent(content) = apply_request(client, view, request).await else { panic!("file content") };
    content
}

async fn sealed_records(client: &mut Client, view: &mut View) -> Vec<v1::NeedsYouRecord> {
    let Body::NeedsYouHistory(history) =
        apply_request(client, view, Request::ListNeedsYouHistory(v1::ListNeedsYouHistory::default())).await
    else {
        panic!("history")
    };
    history.records.into_iter().filter(|record| record.kind() == v1::NeedsYouRecordKind::Sealed).collect()
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

#[tokio::test]
async fn a_sealed_file_says_when_it_changed_and_its_sealed_version_still_opens() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let catalog = view.workspace.extensions.iter().find(|extension| extension.id == "seal").expect("seal");
    assert_eq!((catalog.name.as_str(), catalog.default_enabled), ("Seal", false), "off unless turned on");
    let site = dir.join("site");
    std::fs::create_dir_all(&site).unwrap();
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Tomás", "")).await;
    let tomas = view.workspace.agents.iter().find(|agent| agent.name == "Tomás").expect("Tomás").id.clone();
    let on = v1::ExtensionChange { id: "seal".into(), enabled: Some(true), ..Default::default() };
    apply_request(
        &mut client,
        &mut view,
        Request::UpdateAgent(v1::UpdateAgent { agent_id: tomas.clone(), extension: Some(on), ..Default::default() }),
    )
    .await;
    let file = site.join("passline.md");
    let first = "# Pass line\n\nA pass is at least 40% fewer misses.\n";
    std::fs::write(&file, first).unwrap();
    let path = std::fs::canonicalize(&file).unwrap().display().to_string();
    let (mut watcher, _, _) = Client::connect(addr).await;

    // Unsealed: the read says so.
    let read_now = content(&mut client, &mut view, read(&file, "")).await;
    let seals = read_now.seals.expect("seals on a read from the start");
    assert_eq!((seals.path.as_str(), seals.seals.len(), seals.state()), (path.as_str(), 0, v1::FileSealState::Unspecified));

    // A note is needed, and a file.
    let said = call(&mut client, &mut view, &tomas, "seal", &json!({"path": "passline.md", "note": " "}).to_string()).await;
    assert!(said.starts_with("Say what the seal fixes") && said.ends_with("Nothing was sealed."), "{said}");
    let said =
        call(&mut client, &mut view, &tomas, "seal", &json!({"path": "passline.md", "note": "Claktory: \u{202E}verified"}).to_string())
            .await;
    assert!(said.starts_with("note can't hold hidden") && said.ends_with("Nothing was sealed."), "{said}");
    let said = call(&mut client, &mut view, &tomas, "seal", &json!({"path": ".", "note": "x"}).to_string()).await;
    assert!(said.contains("isn't a file"), "{said}");

    // Sealed, by a path in its folder: every client hears of it.
    let note = "Pass line for run 12, before it runs.";
    let written = "Pass line for run 12,\n  before it runs. ";
    let said = call(&mut client, &mut view, &tomas, "seal", &json!({"path": "passline.md", "note": written}).to_string()).await;
    let sha = sha256(first.as_bytes());
    assert!(said.starts_with(&format!("Sealed {path} (SHA-256 {sha}) on ")), "{said}");
    assert!(said.contains("A copy of this version is kept") && said.contains("Your tools can't change or remove this seal"), "{said}");
    let heard = watcher.wait_for(|body| if let Body::FileSeals(seals) = body { Some(seals.clone()) } else { None }).await;
    assert_eq!((heard.path.as_str(), heard.state()), (path.as_str(), v1::FileSealState::Matches));
    let seal = &heard.seals[0];
    assert_eq!(
        (seal.sha256.as_str(), seal.agent_id.as_str(), seal.agent_name.as_str(), seal.note.as_str(), seal.kept),
        (sha.as_str(), tomas.as_str(), "Tomás", note, true)
    );
    let read_now = content(&mut client, &mut view, read(&file, "")).await;
    assert_eq!(read_now.seals.expect("seals").state(), v1::FileSealState::Matches);
    let kept = v1::ReadFile { path: file.display().to_string(), unless_sha256: sha.clone(), ..Default::default() };
    let again = content(&mut client, &mut view, Request::ReadFile(kept)).await;
    assert!(again.unchanged && again.data.is_empty());
    assert_eq!(again.seals.expect("an unchanged answer has seals too").state(), v1::FileSealState::Matches);

    // Changed since: the read and seal_check say so.
    let second = "# Pass line\n\nA pass is at least 30% fewer misses.\n";
    std::fs::write(&file, second).unwrap();
    let read_now = content(&mut client, &mut view, read(&file, "")).await;
    let seals = read_now.seals.expect("seals");
    assert_eq!((seals.state(), seals.seals.len()), (v1::FileSealState::Changed, 1));
    let said = call(&mut client, &mut view, &tomas, "seal_check", &json!({"path": path}).to_string()).await;
    assert!(said.starts_with(&format!("{path} changed since its latest seal, by Tomás on ")), "{said}");
    assert!(said.contains(&sha) && said.contains(note), "{said}");

    // The version sealed opens as the file did, and only a version sealed.
    let sealed = content(&mut client, &mut view, read(&file, &sha)).await;
    assert_eq!(sealed.error, "");
    assert_eq!((sealed.data.as_slice(), sealed.mime.as_str(), sealed.sha256.as_str()), (first.as_bytes(), "text/markdown", sha.as_str()));
    assert_eq!(sealed.path, file.display().to_string());
    let unsealed = content(&mut client, &mut view, read(&file, &sha256(second.as_bytes()))).await;
    assert_eq!(unsealed.error, "This sealed version wasn't kept.");
    let elsewhere = site.join("other.md");
    std::fs::write(&elsewhere, first).unwrap();
    let elsewhere = content(&mut client, &mut view, read(&elsewhere, &sha)).await;
    assert_eq!(elsewhere.error, "This sealed version wasn't kept.", "a seal opens only its own file's version");

    // The person deletes the copy: the seal stays, and the version no
    // longer opens. Only a seal of that file names what it deletes.
    let delete = |path: &std::path::Path, sha: &str| {
        Request::DeleteSealedCopy(v1::DeleteSealedCopy { path: path.display().to_string(), sha256: sha.to_owned() })
    };
    let refused = apply_request(&mut client, &mut view, delete(&site.join("other.md"), &sha)).await;
    assert!(matches!(&refused, Body::Error(error) if error.message == "This sealed version wasn't kept."), "{refused:?}");
    let deleted = apply_request(&mut client, &mut view, delete(&file, &sha)).await;
    assert!(matches!(deleted, Body::Ack(_)), "{deleted:?}");
    let heard = watcher.wait_for(|body| if let Body::FileSeals(seals) = body { Some(seals.clone()) } else { None }).await;
    assert_eq!((heard.seals.len(), heard.seals[0].kept, heard.seals[0].note.as_str()), (1, false, note));
    let gone = content(&mut client, &mut view, read(&file, &sha)).await;
    assert_eq!(gone.error, "This sealed version wasn't kept.");
    assert!(!home_of(addr).root().join("annotations/blobs").join(&sha).exists(), "the copy itself is gone");

    // Sealed again: both seals stay, and it matches the latest.
    let said = call(&mut client, &mut view, &tomas, "seal", &json!({"path": path, "note": "Corrected before run 13."}).to_string()).await;
    assert!(said.contains("It had one seal before; that one stays."), "{said}");
    let said = call(&mut client, &mut view, &tomas, "seal_check", &json!({"path": "passline.md"}).to_string()).await;
    assert!(said.starts_with(&format!("{path} matches its latest seal, by Tomás")), "{said}");
    assert_eq!(said.matches("\n- ").count(), 2, "{said}");
    // That content sealed again keeps a copy again; the earlier seal doesn't.
    std::fs::write(&file, first).unwrap();
    call(&mut client, &mut view, &tomas, "seal", &json!({"path": "passline.md", "note": "Back to 40%."}).to_string()).await;
    let read_now = content(&mut client, &mut view, read(&file, "")).await;
    let kept: Vec<bool> = read_now.seals.expect("seals").seals.iter().map(|seal| seal.kept).collect();
    assert_eq!(kept, [true, true, false]);
    assert_eq!(content(&mut client, &mut view, read(&file, &sha)).await.data, first.as_bytes());

    // History: Claktory's sentence, the note, the file and the call.
    let records = sealed_records(&mut client, &mut view).await;
    let shown: Vec<_> =
        records.iter().map(|record| (record.title.as_str(), record.agent_reason.as_str(), record.sealed_path.as_str())).collect();
    assert_eq!(
        shown,
        [
            ("Tomás sealed passline.md", "Back to 40%.", path.as_str()),
            ("Tomás sealed passline.md", "Corrected before run 13.", path.as_str()),
            ("Tomás sealed passline.md", note, path.as_str())
        ]
    );
    assert!(records.iter().all(|record| record.outcome() == v1::NeedsYouOutcome::Changed && record.agent_id == tomas));
    let call_entry = view.entries.iter().find(|entry| entry.seq == records[2].seq).expect("its call");
    assert!(matches!(&call_entry.kind, Some(Kind::ToolCall(call)) if call.name == "seal"));

    // The records are never changed or removed.
    let db = rusqlite::Connection::open(home_of(addr).db_path()).unwrap();
    let changed = db.execute("UPDATE seals SET sha256 = 'x'", []);
    assert!(changed.unwrap_err().to_string().contains("a seal is never changed"));
    let removed = db.execute("DELETE FROM seals", []);
    assert!(removed.unwrap_err().to_string().contains("a seal is never removed"));

    // Its agent deleted, History still names it as it was.
    apply_request(&mut client, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: tomas.clone() })).await;
    let records = sealed_records(&mut client, &mut view).await;
    assert!(records.iter().all(|record| record.agent_name == "Tomás"), "{records:?}");
}

#[tokio::test]
async fn claktorys_own_folders_are_never_sealed() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Ana", "")).await;
    let on = v1::ExtensionChange { id: "seal".into(), enabled: Some(true), ..Default::default() };
    apply_request(
        &mut client,
        &mut view,
        Request::UpdateAgent(v1::UpdateAgent { agent_id: "ana".into(), extension: Some(on), ..Default::default() }),
    )
    .await;
    let ink = home_of(addr).root().join("annotations/blobs");
    std::fs::create_dir_all(&ink).unwrap();
    std::fs::write(ink.join("drawing"), "ink").unwrap();
    let said = call(&mut client, &mut view, "ana", "seal", &json!({"path": ink.join("drawing"), "note": "x"}).to_string()).await;
    assert!(said.contains("is in one of Claktory's own folders") && said.ends_with("Nothing was sealed."), "{said}");
}
