//! Files the person uploads for an agent and attaches to a message: sent in
//! chunks, resumed after a reconnect, capped, never overwriting, and read by
//! the agent by path (and as an image, when its model takes them).

use std::path::Path;

use crate::common::{Client, OTHER_MODEL, Scripted, View, apply_request, create_agent, start_with, temp_dir};
use factory_protocol::v1::{self, client_message, entry::Kind, server_message::Body};
use sha2::{Digest, Sha256};

const MIB: usize = 1024 * 1024;

fn start(id: &str, agent: &str, name: &str, size: u64, mime: &str) -> client_message::Body {
    client_message::Body::StartUpload(v1::StartUpload {
        upload_id: id.into(),
        agent_id: agent.into(),
        name: name.into(),
        size,
        mime: mime.into(),
    })
}

fn chunk(id: &str, offset: usize, data: &[u8]) -> client_message::Body {
    client_message::Body::UploadChunk(v1::UploadChunk { upload_id: id.into(), offset: offset as u64, data: data.to_vec() })
}

fn finish(id: &str, sha256: &str) -> client_message::Body {
    client_message::Body::FinishUpload(v1::FinishUpload { upload_id: id.into(), sha256: sha256.into() })
}

fn hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// How much of the upload the backend has, or the error's message.
async fn progress(client: &mut Client, body: client_message::Body) -> Result<u64, String> {
    match client.request(body).await.body {
        Body::UploadProgress(progress) => Ok(progress.received),
        Body::Error(error) => Err(error.message),
        other => panic!("expected progress, got {other:?}"),
    }
}

async fn finished(client: &mut Client, body: client_message::Body) -> Result<v1::Attachment, String> {
    match client.request(body).await.body {
        Body::UploadedFile(file) => Ok(file.attachment.expect("the file")),
        Body::Error(error) => Err(error.message),
        other => panic!("expected the file, got {other:?}"),
    }
}

/// Uploads `data` whole, in one go.
async fn upload(client: &mut Client, id: &str, agent: &str, name: &str, data: &[u8], mime: &str) -> v1::Attachment {
    assert_eq!(progress(client, start(id, agent, name, data.len() as u64, mime)).await, Ok(0));
    let mut offset = 0;
    for part in data.chunks(MIB) {
        offset += part.len();
        assert_eq!(progress(client, chunk(id, offset - part.len(), part)).await, Ok(offset as u64));
    }
    finished(client, finish(id, &hex(data))).await.expect("stored")
}

fn partials(dir: &Path, agent: &str) -> usize {
    std::fs::read_dir(dir.join(format!("home/agents/{agent}/uploads/.partial"))).map_or(0, Iterator::count)
}

#[tokio::test]
async fn files_upload_in_chunks_resume_after_a_reconnect_and_never_overwrite() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;

    // 2.5 MiB in three chunks; the connection drops after the first.
    let data: Vec<u8> = (0..(5 * MIB / 2)).map(|n| (n % 251) as u8).collect();
    let size = data.len() as u64;
    assert_eq!(progress(&mut client, start("u1", "ops", "../../report.bin", size, "")).await, Ok(0));
    assert_eq!(progress(&mut client, chunk("u1", 0, &data[..MIB])).await, Ok(MIB as u64));
    drop(client);
    let (mut client, _, _) = Client::connect(addr).await;
    // Starting it again says how much is there; it goes on from there only.
    assert_eq!(progress(&mut client, start("u1", "ops", "../../report.bin", size, "")).await, Ok(MIB as u64));
    let wrong = progress(&mut client, chunk("u1", 0, &data[..MIB])).await;
    assert_eq!(wrong, Err(format!("The upload goes on from {MIB} bytes, not 0.")));
    let big = vec![0; MIB + 1];
    assert_eq!(progress(&mut client, chunk("u1", MIB, &big)).await, Err("A chunk can be at most 1 MiB.".into()));
    let early = finished(&mut client, finish("u1", "")).await;
    assert_eq!(early, Err(format!("report.bin isn't all there yet: {MIB} of {size} bytes.")));
    assert_eq!(progress(&mut client, chunk("u1", MIB, &data[MIB..2 * MIB])).await, Ok(2 * MIB as u64));
    assert_eq!(progress(&mut client, chunk("u1", 2 * MIB, &data[2 * MIB..])).await, Ok(size));
    let stored = finished(&mut client, finish("u1", &hex(&data))).await.expect("stored");
    // In the agent's uploads, under today, by its own name only.
    let path = Path::new(&stored.path);
    let day = path.parent().and_then(Path::file_name).and_then(|day| day.to_str()).expect("a day").to_owned();
    assert!(day.len() == 10 && day.as_bytes()[4] == b'-' && day.as_bytes()[7] == b'-', "{day}");
    let uploads = dir.join("home/agents/ops/uploads");
    assert_eq!(path, uploads.join(&day).join("report.bin"));
    assert_eq!((stored.name.as_str(), stored.size), ("report.bin", size));
    assert_eq!(std::fs::read(path).expect("the file"), data);
    assert_eq!(partials(&dir, "ops"), 0, "nothing left half done");
    // It's done: more of it is refused.
    assert!(progress(&mut client, chunk("u1", 0, b"x")).await.is_err());

    // The same name again doesn't overwrite it.
    let again = upload(&mut client, "u2", "ops", "report.bin", b"second", "").await;
    assert_eq!(again.name, "report 2.bin");
    assert_eq!(std::fs::read(&stored.path).expect("the first"), data);

    // Too big, damaged, cancelled, or for no one.
    let huge = progress(&mut client, start("u3", "ops", "disk.img", 3_000_000_000, "")).await;
    assert_eq!(huge, Err("disk.img is 3.0 GB; files can be up to 2 GB.".into()));
    assert_eq!(progress(&mut client, start("u4", "ops", "notes.txt", 5, "text/plain")).await, Ok(0));
    assert_eq!(progress(&mut client, chunk("u4", 0, b"hello")).await, Ok(5));
    let damaged = finished(&mut client, finish("u4", &hex(b"other"))).await;
    assert_eq!(damaged, Err("notes.txt arrived damaged (its SHA-256 differs); send it again.".into()));
    assert_eq!(partials(&dir, "ops"), 0);
    assert_eq!(progress(&mut client, start("u5", "ops", "notes.txt", 5, "")).await, Ok(0));
    assert_eq!(progress(&mut client, chunk("u5", 0, b"hel")).await, Ok(3));
    assert_eq!(partials(&dir, "ops"), 1);
    let cancel = client_message::Body::CancelUpload(v1::CancelUpload { upload_id: "u5".into() });
    assert!(matches!(client.request(cancel).await.body, Body::Ack(_)));
    assert_eq!(partials(&dir, "ops"), 0);
    assert!(progress(&mut client, chunk("u5", 3, b"lo")).await.is_err());
    let nobody = progress(&mut client, start("u6", "nobody", "a.txt", 1, "")).await;
    assert_eq!(nobody, Err("no agent with id \"nobody\"".into()));
    assert_eq!(progress(&mut client, start("u7", "ops", "..", 1, "")).await, Err("The file needs a name.".into()));
}

#[tokio::test]
async fn a_message_carries_its_files_by_path_and_images_too_when_the_model_reads_them() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "ops".into() });
    apply_request(&mut client, &mut view, select).await;
    let last = || requests.lock().expect("lock").last().map(|request| (request.prompt.clone(), request.images.clone())).expect("a request");
    let send = |text: &str, attachments: Vec<v1::Attachment>| {
        client_message::Body::SendMessage(v1::SendMessage { agent_id: "ops".into(), text: text.into(), attachments, ..Default::default() })
    };
    let idle = |view: &View| view.agent("ops").status() == v1::AgentStatus::Idle && view.agent("ops").queued_messages == 0;

    let image = upload(&mut client, "p1", "ops", "chart.png", b"\x89PNG not really", "image/png").await;
    let notes = upload(&mut client, "p2", "ops", "notes.txt", b"hello", "text/plain").await;

    // Files only: the model reads a tag for each; Echo doesn't take images.
    apply_request(&mut client, &mut view, send("", vec![image.clone(), notes.clone()])).await;
    view.follow(&mut client, |view| view.entries.len() >= 2 && idle(view)).await;
    let (prompt, images) = last();
    let tag = |file: &v1::Attachment, kind: &str| {
        format!("<attachment path=\"{}\" name=\"{}\" size=\"{}\" type=\"{kind}\"/>", file.path, file.name, file.size)
    };
    // Inside the tag naming the device it came from.
    assert!(prompt.starts_with("<sent from=\""), "{prompt}");
    assert!(prompt.ends_with(&format!(">\n{}\n{}\n</sent>", tag(&image, "image/png"), tag(&notes, "text/plain"))), "{prompt}");
    assert!(images.is_empty());
    let Some(Kind::User(user)) = &view.entries[0].kind else { panic!("the person's message") };
    assert_eq!((user.text.as_str(), user.attachments.clone()), ("", vec![image.clone(), notes.clone()]));

    // Parrot takes images: it gets the image itself too, after the text.
    let parrot = v1::UpdateAgent { agent_id: "ops".into(), model: Some(OTHER_MODEL.into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(parrot)).await;
    apply_request(&mut client, &mut view, send("What does it show?", vec![image.clone()])).await;
    view.follow(&mut client, |view| view.entries.len() >= 4 && idle(view)).await;
    let (prompt, images) = last();
    assert!(prompt.ends_with(&format!("What does it show?\n{}\n</sent>", tag(&image, "image/png"))), "{prompt}");
    assert_eq!(images, ["image/png"]);
    let user = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::User(user)) => Some(user.clone()),
        _ => None,
    });
    assert_eq!(user.map(|user| user.text), Some("What does it show?".to_owned()), "the image shows as an attachment, not as text");

    // Only its own uploads, still there.
    let elsewhere = v1::Attachment { path: dir.join("home/config.json").display().to_string(), ..image.clone() };
    let refused = client.request(send("Look", vec![elsewhere])).await.body;
    assert!(matches!(&refused, Body::Error(error) if error.message.ends_with("isn't one of this agent's uploads.")), "{refused:?}");
    std::fs::remove_file(&notes.path).expect("moved away");
    let gone = client.request(send("Look", vec![notes])).await.body;
    assert!(matches!(&gone, Body::Error(error) if error.message.ends_with("isn't one of this agent's uploads.")), "{gone:?}");
    let empty = client.request(send("  ", Vec::new())).await.body;
    assert!(matches!(&empty, Body::Error(_)), "{empty:?}");
}
