//! Ink: blobs upload privately, by their SHA-256, for no
//! agent; no agent's file tools reach where they're kept, admins' included;
//! and nothing refers to a blob yet, so none can be read back.

use std::path::Path;

use crate::common::{Client, OTHER_MODEL, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message::Body as Request, entry::Kind, server_message::Body};
use sha2::{Digest, Sha256};

fn hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn start_ink(id: &str, agent: &str, size: u64, mime: &str) -> Request {
    Request::StartUpload(v1::StartUpload {
        upload_id: id.into(),
        agent_id: agent.into(),
        name: if mime == "image/png" { "preview.png" } else { "drawing.pkdrawing" }.into(),
        size,
        mime: mime.into(),
        purpose: v1::UploadPurpose::Ink.into(),
    })
}

async fn reply(client: &mut Client, body: Request) -> Result<Body, String> {
    match client.request(body).await.body {
        Body::Error(error) => Err(error.message),
        other => Ok(other),
    }
}

/// Uploads `data` as ink, finishing with `sha`: the blob's name, or why not.
async fn upload(client: &mut Client, id: &str, data: &[u8], mime: &str, sha: &str) -> Result<String, String> {
    reply(client, start_ink(id, "", data.len() as u64, mime)).await?;
    let chunk = v1::UploadChunk { upload_id: id.into(), offset: 0, data: data.to_vec() };
    reply(client, Request::UploadChunk(chunk)).await?;
    match reply(client, Request::FinishUpload(v1::FinishUpload { upload_id: id.into(), sha256: sha.into() })).await? {
        Body::UploadedFile(file) => {
            assert!(file.attachment.is_none(), "ink is no agent's attachment");
            Ok(file.blob_sha256)
        }
        other => panic!("expected the file, got {other:?}"),
    }
}

fn staged(home: &Path) -> usize {
    std::fs::read_dir(home.join("annotations/staging")).map_or(0, Iterator::count)
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0]);
    png
}

#[tokio::test]
async fn ink_blobs_upload_privately_by_their_hash_and_are_kept_once() {
    let dir = temp_dir();
    let home = dir.join("home");
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;

    // For no agent, of a known kind, within its limit.
    let refused = |result: Result<Body, String>| result.expect_err("refused");
    assert!(refused(reply(&mut client, start_ink("u1", "ops", 10, "application/x-pencilkit")).await).contains("no agent"));
    assert!(refused(reply(&mut client, start_ink("u1", "", 10, "text/html")).await).contains("not \"text/html\""));
    assert!(refused(reply(&mut client, start_ink("u1", "", 8 * 1024 * 1024 + 1, "application/x-pencilkit")).await).contains("8 MiB"));
    assert!(refused(reply(&mut client, start_ink("u1", "", 4 * 1024 * 1024 + 1, "image/png")).await).contains("4 MiB"));

    // Never kept without its hash, or with another; nothing staged is left.
    let drawing: Vec<u8> = (0..3000).map(|n| (n % 251) as u8).collect();
    let sha = hex(&drawing);
    assert!(upload(&mut client, "u2", &drawing, "application/x-pencilkit", "").await.unwrap_err().contains("needs its SHA-256"));
    assert!(upload(&mut client, "u3", &drawing, "application/x-pencilkit", &hex(b"other")).await.unwrap_err().contains("damaged"));
    assert_eq!(staged(&home), 0);
    assert!(!home.join("annotations/blobs").join(&sha).exists());

    // Kept by its hash, private, and the same again at once.
    assert_eq!(upload(&mut client, "u4", &drawing, "application/x-pencilkit", &sha.to_uppercase()).await, Ok(sha.clone()));
    let blob = home.join(format!("annotations/blobs/{sha}"));
    assert_eq!(std::fs::read(&blob).expect("kept"), drawing);
    assert_eq!(crate::common::mode(&blob), 0o600);
    assert_eq!(crate::common::mode(&home.join("annotations")), 0o700);
    assert_eq!(upload(&mut client, "u5", &drawing, "application/x-pencilkit", &sha).await, Ok(sha.clone()));
    assert_eq!(staged(&home), 0);
    assert!(!home.join("agents/ops/uploads").exists(), "nothing reaches an agent's uploads");

    // A PNG is one.
    let not_png = b"GIF89a not a png".to_vec();
    assert!(upload(&mut client, "u6", &not_png, "image/png", &hex(&not_png)).await.unwrap_err().contains("isn't a PNG"));
    let preview = png(2048, 1536);
    assert_eq!(upload(&mut client, "u7", &preview, "image/png", &hex(&preview)).await, Ok(hex(&preview)));

    // Nothing refers to them yet, so nothing reads them back.
    let read = Request::ReadInkBlob(v1::ReadInkBlob { sha256: sha.clone(), offset: 0 });
    match reply(&mut client, read).await {
        Ok(Body::InkBlobData(data)) => assert_eq!((data.error.as_str(), data.data.len()), ("unknown", 0)),
        other => panic!("expected InkBlobData, got {other:?}"),
    }
    let traversal = Request::ReadInkBlob(v1::ReadInkBlob { sha256: "../../config.json".into(), offset: 0 });
    assert!(matches!(reply(&mut client, traversal).await, Ok(Body::InkBlobData(data)) if data.error == "unknown" && data.data.is_empty()));
}

/// What `agent`'s call of `tool` with `args` answered.
async fn tool(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    let done = |view: &View| {
        let it = view.agent(agent);
        view.entries.len() >= before + 3 && it.status() == AgentStatus::Idle && it.queued_messages == 0
    };
    view.follow(client, done).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

#[tokio::test]
async fn no_agents_file_tools_reach_ink_wherever_it_works_admins_included() {
    let dir = temp_dir();
    let home = dir.join("home");
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let sha = "a".repeat(64);
    std::fs::create_dir_all(home.join("annotations/blobs")).expect("mkdir");
    std::fs::write(home.join(format!("annotations/blobs/{sha}")), "INK-CANARY").expect("write");
    std::fs::write(home.join("annotations/preview.png"), png(10, 10)).expect("write");
    // Ada works in Claktory's own folder, so ink is inside her folder; Bo's
    // folder has a link to it.
    apply_request(&mut client, &mut view, create_project("Home", &home)).await;
    apply_request(&mut client, &mut view, create_agent("home", "Ada", "")).await;
    // Her model takes images, so read_image is hers to try, and she's an
    // admin agent, as Rivet is.
    let parrot = v1::UpdateAgent { agent_id: "ada".into(), model: Some(OTHER_MODEL.into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(parrot)).await;
    let value = v1::OptionValue { on: true, ..Default::default() };
    let admin = v1::ExtensionChange { id: "agents".into(), option_id: "admin".into(), value: Some(value), ..Default::default() };
    let admin = v1::UpdateAgent { agent_id: "ada".into(), extension: Some(admin), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(admin)).await;
    let at = view.workspace.extensions.iter().position(|extension| extension.id == "agents").expect("listed");
    assert!(view.agent("ada").extensions[at].options["admin"].on);
    let work = dir.join("work");
    std::fs::create_dir_all(&work).expect("mkdir");
    crate::common::symlink(&home.join("annotations"), &work.join("link")).expect("link");
    apply_request(&mut client, &mut view, create_project("Work", &work)).await;
    apply_request(&mut client, &mut view, create_agent("work", "Bo", "")).await;

    let blob = format!("annotations/blobs/{sha}");
    let absolute = crate::common::json_text(home.join(&blob).display());
    let cases = [
        ("ada", "read_file", format!(r#"{{"path":"{blob}"}}"#)),
        ("ada", "read_file", format!(r#"{{"path":"{absolute}"}}"#)),
        ("ada", "read_file", r#"{"path":"agents/../annotations/blobs"}"#.to_owned()),
        ("ada", "read_image", r#"{"path":"annotations/preview.png"}"#.to_owned()),
        ("ada", "write_file", r#"{"path":"annotations/new.txt","content":"x"}"#.to_owned()),
        ("ada", "edit_file", format!(r#"{{"path":"{blob}","edits":[{{"oldText":"INK","newText":"OUT"}}]}}"#)),
        ("bo", "read_file", format!(r#"{{"path":"link/blobs/{sha}"}}"#)),
        ("bo", "write_file", r#"{"path":"link/staging/x","content":"x"}"#.to_owned()),
    ];
    for (agent, name, args) in cases {
        let said = tool(&mut client, &mut view, agent, name, &args).await;
        assert!(said.contains("Claktory's private storage"), "{agent} {name} {args}: {said}");
        assert!(!said.contains("INK-CANARY"));
    }
    assert_eq!(std::fs::read_to_string(home.join(&blob)).unwrap(), "INK-CANARY");
    assert!(!home.join("annotations/new.txt").exists() && !home.join("annotations/staging/x").exists());
    // The rest of her folder is hers as before.
    let said = tool(&mut client, &mut view, "ada", "write_file", r#"{"path":"notes.txt","content":"fine"}"#).await;
    assert!(!said.contains("private storage"), "{said}");
}
