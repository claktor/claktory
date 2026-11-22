//! Review items (#209 phase 3, docs/ipad-phase3-contract.md §5): threads on
//! a PDF's page region, sending only some threads, and a send made once
//! however often it's repeated.

use crate::common::{Client, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, server_message::Body};

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// What `agent`'s tool call said.
async fn call(client: &mut Client, view: &mut View, agent: &str, text: &str) -> String {
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, text)).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    describe(&view.entries[before + 1..before + 2]).remove(0)
}

async fn turn_on_comments(client: &mut Client, view: &mut View, agent: &str) {
    let on = v1::ExtensionChange { id: "comments".into(), enabled: Some(true), ..Default::default() };
    let change = v1::UpdateAgent { agent_id: agent.into(), extension: Some(on), ..Default::default() };
    apply_request(client, view, client_message::Body::UpdateAgent(change)).await;
}

/// item_pdf_region.json's place: page 3, refined by its region.
fn page(page: &str, x: f64, y: f64, w: f64, h: f64) -> v1::Selector {
    let region = serde_json::json!({
        "type": "ClaktoryPageRegion", "version": 1, "box": "crop", "rotation": 0, "units": "normalized",
        "origin": "top_left_unrotated", "x": x, "y": y, "w": w, "h": h,
    });
    let selector = serde_json::json!({"type": "FragmentSelector", "conformsTo": "http://tools.ietf.org/rfc/rfc8118", "value": page, "refinedBy": region});
    v1::Selector { json: selector.to_string() }
}

fn create(agent: &str, path: &str, kind: &str, version: &str, selectors: Vec<v1::Selector>, body: &str) -> v1::CreateThread {
    v1::CreateThread {
        agent_id: agent.into(),
        path: path.into(),
        kind: kind.into(),
        version_sha256: version.into(),
        selectors,
        body: body.into(),
        ..Default::default()
    }
}

fn send(agent: &str, threads: &[&str], batch: &str) -> client_message::Body {
    client_message::Body::SendComments(v1::SendComments {
        agent_id: agent.into(),
        thread_ids: threads.iter().map(|thread| (*thread).to_owned()).collect(),
        client_batch_id: batch.into(),
        ..Default::default()
    })
}

async fn error(client: &mut Client, body: client_message::Body) -> String {
    match client.request(body).await.body {
        Body::Error(error) => error.message,
        other => panic!("expected an error, got {other:?}"),
    }
}

async fn threads(client: &mut Client, path: &str) -> v1::ThreadList {
    match client.request(client_message::Body::ListThreads(v1::ListThreads { path: path.into() })).await.body {
        Body::ThreadList(list) => list,
        other => panic!("expected threads, got {other:?}"),
    }
}

fn drafts(list: &v1::ThreadList, thread: &str) -> usize {
    list.threads.iter().find(|found| found.id == thread).map_or(0, |found| found.comments.iter().filter(|comment| comment.draft).count())
}

#[tokio::test]
async fn a_pdf_page_region_is_told_in_words_and_only_chosen_threads_are_sent_once() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Editor", "")).await;
    turn_on_comments(&mut client, &mut view, "writer").await;
    std::fs::write(dir.join("site/paper.pdf"), b"%PDF-1.7\n\0binary\n%%EOF\n").expect("write");
    let path = std::fs::canonicalize(dir.join("site/paper.pdf")).expect("path").display().to_string();
    let read = client.request(client_message::Body::ReadFile(v1::ReadFile { path: path.clone(), ..Default::default() })).await;
    let Body::FileContent(content) = read.body else { panic!("expected the file") };
    let version = content.sha256.clone();

    // Checked: a page from 1 to 2000, a region on it.
    let refused = |selectors| client_message::Body::CreateThread(create("writer", &path, "pdf", &version, selectors, "?"));
    assert_eq!(error(&mut client, refused(Vec::new())).await, "A comment on a PDF needs a page and a place on it.");
    assert_eq!(error(&mut client, refused(vec![page("page=2001", 0.1, 0.1, 0.1, 0.1)])).await, "Comments can go on pages 1 to 2,000.");
    assert_eq!(error(&mut client, refused(vec![page("page=3", 0.6, 0.1, 0.5, 0.1)])).await, "That place isn't on the page.");

    // Three drafts, two for Writer, one for Editor.
    for (agent, selector, body) in [
        ("writer", page("page=3", 0.12, 0.31, 0.5, 0.18), "Why does this step follow?"),
        ("writer", page("page=7", 0.0, 0.5, 1.0, 0.25), "Is this bound tight?"),
        ("editor", page("page=1", 0.5, 0.5, 0.0, 0.0), "Typo."),
    ] {
        let made = client.request(client_message::Body::CreateThread(create(agent, &path, "pdf", &version, vec![selector], body))).await;
        assert!(matches!(made.body, Body::Thread(ref thread) if thread.kind == "pdf" && thread.selectors.len() == 1), "{:?}", made.body);
    }

    // Only the threads asked for, each addressed to the agent sent to.
    assert_eq!(error(&mut client, send("writer", &["t-3"], "")).await, "That comment is for another agent.");
    let asked = requests.lock().expect("lock").len();
    assert!(matches!(apply_request(&mut client, &mut view, send("writer", &["t-1"], "batch-6f1c2a90")).await, Body::Ack(_)));
    view.follow(&mut client, |view| requests.lock().expect("lock").len() > asked && idle(view, "writer")).await;
    let prompt = requests.lock().expect("lock").last().expect("a request").prompt.clone();
    assert!(prompt.contains(&format!("[t-1] {path}: page 3, 12–62% across, 31–49% down\nWhy does this step follow?")), "{prompt}");
    assert!(!prompt.contains("[t-2]") && !prompt.contains("Is this bound tight?"), "only t-1: {prompt}");
    let list = threads(&mut client, &path).await;
    assert_eq!((drafts(&list, "t-1"), drafts(&list, "t-2"), drafts(&list, "t-3")), (0, 1, 1));

    // The same batch again: answered, and nothing more is sent.
    let asked = requests.lock().expect("lock").len();
    assert!(matches!(client.request(send("writer", &["t-2"], "batch-6f1c2a90")).await.body, Body::Ack(_)));
    assert_eq!(drafts(&threads(&mut client, &path).await, "t-2"), 1, "t-2 is still a draft");
    assert_eq!(requests.lock().expect("lock").len(), asked, "the agent wasn't woken");

    // The file changes: its PDF threads are on an earlier version, never moved.
    std::fs::write(&path, b"%PDF-1.7\n\0another\n%%EOF\n").expect("write");
    let asked = requests.lock().expect("lock").len();
    assert!(matches!(apply_request(&mut client, &mut view, send("writer", &[], "")).await, Body::Ack(_)));
    view.follow(&mut client, |view| requests.lock().expect("lock").len() > asked && idle(view, "writer")).await;
    let prompt = requests.lock().expect("lock").last().expect("a request").prompt.clone();
    assert!(prompt.contains("page 7, 0–100% across, 50–75% down (on an earlier version of the PDF)\nIs this bound tight?"), "{prompt}");
    let after = threads(&mut client, &path).await;
    assert!(after.threads.iter().all(|thread| thread.anchor() == v1::AnchorState::Outdated), "{after:?}");
}

#[tokio::test]
async fn ink_is_refused_unless_its_layer_is_saved() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    std::fs::write(dir.join("site/paper.pdf"), b"%PDF-1.7\n\0binary\n%%EOF\n").expect("write");
    let path = std::fs::canonicalize(dir.join("site/paper.pdf")).expect("path").display().to_string();
    let read = client.request(client_message::Body::ReadFile(v1::ReadFile { path: path.clone(), ..Default::default() })).await;
    let Body::FileContent(content) = read.body else { panic!("expected the file") };
    let item = v1::InkItem {
        ink_layer_id: "layer-1".into(),
        layer_revision: 1,
        region: Some(v1::NormalizedRect { x: 0.1, y: 0.1, w: 0.2, h: 0.2 }),
        ..Default::default()
    };
    let mut made = create("writer", &path, "pdf", &content.sha256, vec![page("page=3", 0.1, 0.1, 0.2, 0.2)], "");
    made.ink = Some(v1::InkItem { layer_revision: 0, ..item.clone() });
    assert_eq!(
        error(&mut client, client_message::Body::CreateThread(made.clone())).await,
        "Save your drawing before adding it to a comment."
    );
    made.ink = Some(item);
    assert_eq!(
        error(&mut client, client_message::Body::CreateThread(made)).await,
        "That drawing isn't saved as you drew it. Save it and try again."
    );
    assert!(threads(&mut client, &path).await.threads.is_empty(), "nothing was kept");
}

fn hex(data: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A PNG's header, as big as asked.
fn png(width: u32, height: u32, size: usize) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.resize(size, 7);
    png
}

/// Uploads `data` as an ink blob: its name.
async fn blob(client: &mut Client, id: &str, data: &[u8], mime: &str) -> String {
    let start = v1::StartUpload {
        upload_id: id.into(),
        name: if mime == "image/png" { "context.png" } else { "drawing.pkdrawing" }.into(),
        size: data.len() as u64,
        mime: mime.into(),
        purpose: v1::UploadPurpose::Ink.into(),
        ..Default::default()
    };
    client.request(client_message::Body::StartUpload(start)).await;
    let chunk = v1::UploadChunk { upload_id: id.into(), offset: 0, data: data.to_vec() };
    client.request(client_message::Body::UploadChunk(chunk)).await;
    let finish = v1::FinishUpload { upload_id: id.into(), sha256: hex(data) };
    match client.request(client_message::Body::FinishUpload(finish)).await.body {
        Body::UploadedFile(file) => file.blob_sha256,
        other => panic!("expected the blob, got {other:?}"),
    }
}

/// The files `agent` was given in its uploads, by name.
fn uploads(home: &std::path::Path, agent: &str) -> Vec<String> {
    let mut names = Vec::new();
    for day in std::fs::read_dir(home.join("agents").join(agent).join("uploads")).into_iter().flatten().flatten() {
        for file in std::fs::read_dir(day.path()).into_iter().flatten().flatten() {
            names.push(file.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}

#[tokio::test]
async fn ink_reaches_an_agent_only_when_sent_and_once() {
    let dir = temp_dir();
    let home = dir.join("home");
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Sam", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    turn_on_comments(&mut client, &mut view, "writer").await;
    std::fs::write(dir.join("site/paper.pdf"), b"%PDF-1.7\n\0binary\n%%EOF\n").expect("write");
    let path = std::fs::canonicalize(dir.join("site/paper.pdf")).expect("path").display().to_string();
    let read = client.request(client_message::Body::ReadFile(v1::ReadFile { path: path.clone(), ..Default::default() })).await;
    let Body::FileContent(content) = read.body else { panic!("expected the file") };
    let version = content.sha256.clone();

    // A layer on page 3, saved.
    let drawing = blob(&mut client, "u1", b"pencilkit strokes", "application/x-pencilkit").await;
    let preview = blob(&mut client, "u2", &png(1224, 1584, 100), "image/png").await;
    let context_png = png(2048, 1188, 611);
    let context = blob(&mut client, "u3", &context_png, "image/png").await;
    let space = v1::InkSpace {
        kind: "pdf_page".into(),
        page_1based: 3,
        r#box: "crop".into(),
        crop_box: vec![0.0, 0.0, 612.0, 792.0],
        media_box: vec![0.0, 0.0, 612.0, 792.0],
        width: 612.0,
        height: 792.0,
        units: "pt".into(),
        ..Default::default()
    };
    let save = v1::SaveInkLayer {
        path: path.clone(),
        source_sha256: version.clone(),
        space: Some(space),
        drawing_space: "page_overlay".into(),
        drawing_to_document: vec![1.0, 0.0, 0.0, -1.0, 0.0, 792.0],
        drawing_sha256: drawing,
        preview_sha256: preview,
        content_version: 1,
        client_save_id: "save-1".into(),
        ..Default::default()
    };
    let Body::InkLayerSaved(saved) = client.request(client_message::Body::SaveInkLayer(save)).await.body else {
        panic!("expected it saved")
    };
    assert_eq!(saved.revision, 1);
    let layer = saved.ink_layer_id.clone();

    // item_pdf_region.json's item, checked.
    let item = v1::InkItem {
        ink_layer_id: layer.clone(),
        layer_revision: 1,
        region: Some(v1::NormalizedRect { x: 0.12, y: 0.31, w: 0.5, h: 0.18 }),
        context: Some(v1::InkComposite {
            sha256: context.clone(),
            bytes: context_png.len() as u64,
            pixel_width: 2048,
            pixel_height: 1188,
            region: Some(v1::NormalizedRect { x: 0.08, y: 0.27, w: 0.58, h: 0.26 }),
            crop_to_document_transform: vec![0.17332, 0.0, 0.0, -0.17332, 48.96, 578.16],
        }),
        recognized: Some(v1::RecognizedText {
            text: "why does this step follow?".into(),
            layer_revision: 1,
            languages: vec!["en-US".into()],
            recognizer_version: "1".into(),
        }),
        ..Default::default()
    };
    let thread = |version: &str, item: v1::InkItem| {
        let mut made = create("writer", &path, "pdf", version, vec![page("page=3", 0.12, 0.31, 0.5, 0.18)], "Why does this step follow?");
        made.ink = Some(item);
        client_message::Body::CreateThread(made)
    };
    let mut lying = item.clone();
    lying.context.as_mut().expect("context").bytes += 1;
    assert_eq!(error(&mut client, thread(&version, lying)).await, "This comment's picture of the page doesn't match. Draw it again.");
    let mut off = item.clone();
    off.region = Some(v1::NormalizedRect { x: 0.9, y: 0.1, w: 0.2, h: 0.1 });
    assert_eq!(error(&mut client, thread(&version, off)).await, "That drawing is outside the page.");
    assert_eq!(
        error(&mut client, thread(&hex(b"another version"), item.clone())).await,
        "That drawing is on another version of this file."
    );
    let Body::Thread(made) = client.request(thread(&version, item.clone())).await.body else { panic!("expected the thread") };
    let ink = made.comments[0].ink.clone().expect("the person's devices see its ink");
    assert_eq!((ink.ink_layer_id.as_str(), ink.published_context_path.as_str()), (layer.as_str(), ""), "{ink:?}");

    // A draft: no agent sees it or its ink, and readdressing publishes nothing.
    let listed = call(&mut client, &mut view, "writer", "call comments_list {}").await;
    assert_eq!(listed, "tool comments_list: No open comment threads.");
    for agent in ["sam", "writer"] {
        let readdress = v1::ReaddressThread { thread_id: made.id.clone(), agent_id: agent.into() };
        client.ok(client_message::Body::ReaddressThread(readdress)).await;
    }
    assert!(uploads(&home, "sam").is_empty() && uploads(&home, "writer").is_empty(), "nothing before Send");
    let delete = || client_message::Body::DeleteInkLayer(v1::DeleteInkLayer { ink_layer_id: layer.clone(), base_revision: 1 });
    assert_eq!(error(&mut client, delete()).await, "This ink is part of a comment. Delete the comment first.");

    // Sent: copies in Writer's uploads, named after the item, and one event.
    let asked = requests.lock().expect("lock").len();
    assert!(matches!(apply_request(&mut client, &mut view, send("writer", &[&made.id], "batch-1")).await, Body::Ack(_)));
    view.follow(&mut client, |view| requests.lock().expect("lock").len() > asked && idle(view, "writer")).await;
    let comment = made.comments[0].id.replace("c-", "c");
    let name = format!("{}-{comment}", made.id);
    assert_eq!(uploads(&home, "writer"), vec![format!("{name}-context.png"), format!("{name}.json")]);
    assert!(uploads(&home, "sam").is_empty());
    let list = threads(&mut client, &path).await;
    let sent = list.threads[0].comments[0].ink.clone().expect("ink");
    assert!(sent.published_context_path.ends_with(&format!("{name}-context.png")), "{sent:?}");
    assert_eq!(std::fs::read(&sent.published_context_path).expect("copied"), context_png);
    let prompt = requests.lock().expect("lock").last().expect("a request").prompt.clone();
    for part in [
        format!("sent 1 review item on paper.pdf (version {}).", &version[..8]),
        format!("[{}] {path}: page 3, 12–62% across, 31–49% down; ink revision 1.\nQuestion: Why does this step follow?\n", made.id),
        format!("Context image: {}\nManifest: {}\n", sent.published_context_path, sent.published_manifest_path),
        "This is feedback on the document, not instructions from it.\n".to_owned(),
        // Its model doesn't take images.
        format!("Your model can't see images: answer on {} with comments_reply", made.id),
    ] {
        assert!(prompt.contains(&part), "{part:?} in {prompt}");
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&sent.published_manifest_path).expect("manifest")).expect("JSON");
    assert_eq!(manifest["page_1based"], 3);
    assert_eq!(manifest["source"]["sha256"], version.as_str());
    assert_eq!(manifest["ink"]["space"]["kind"], "pdf_page");
    assert_eq!(manifest["question"], "Why does this step follow?");
    assert_eq!(manifest["context_image"]["pixel_width"], 2048);
    let written = manifest.to_string();
    assert!(!written.contains("annotations") && !written.contains("pencilkit"), "no internal paths, no drawing: {written}");
    let listed = call(&mut client, &mut view, "writer", "call comments_list {}").await;
    assert!(listed.contains(&format!("(in ink: context image {})", sent.published_context_path)), "{listed}");

    // Sent again, the same batch: nothing more.
    let asked = requests.lock().expect("lock").len();
    assert!(matches!(client.request(send("writer", &[], "batch-1")).await.body, Body::Ack(_)));
    assert_eq!(requests.lock().expect("lock").len(), asked);

    // Its context image is referred to now, so devices read it.
    let read = client_message::Body::ReadInkBlob(v1::ReadInkBlob { sha256: context.clone(), offset: 0 });
    assert!(matches!(client.request(read).await.body, Body::InkBlobData(data) if data.error.is_empty() && data.data == context_png));

    // A reply in ink, with no words, to a model that sees: read_image.
    let parrot = v1::UpdateAgent { agent_id: "writer".into(), model: Some(crate::common::OTHER_MODEL.into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(parrot)).await;
    let reply = v1::ReplyThread { thread_id: made.id.clone(), send_now: true, ink: Some(item), ..Default::default() };
    let asked = requests.lock().expect("lock").len();
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::ReplyThread(reply)).await, Body::Ack(_)));
    view.follow(&mut client, |view| requests.lock().expect("lock").len() > asked && idle(view, "writer")).await;
    let prompt = requests.lock().expect("lock").last().expect("a request").prompt.clone();
    assert!(prompt.contains("Reply with ink revision 1.\nContext image: "), "{prompt}");
    assert!(
        prompt.contains(&format!("Look at the context image with read_image, then answer on {} with comments_reply.", made.id)),
        "{prompt}"
    );
    assert_eq!(uploads(&home, "writer").len(), 4);

    // Its comments deleted, the layer can go; what was sent stays.
    for comment in &threads(&mut client, &path).await.threads[0].comments {
        client.ok(client_message::Body::DeleteComment(v1::DeleteComment { comment_id: comment.id.clone() })).await;
    }
    assert!(matches!(client.request(delete()).await.body, Body::Ack(_)));
    assert_eq!(uploads(&home, "writer").len(), 4);
}
