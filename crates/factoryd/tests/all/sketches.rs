//! Sketches (#209 phase 3 §6): a blank white PNG in one of the person's
//! projects' folders, never over anything there, then an ordinary image
//! with an ink layer on it.

use crate::common::{Client, View, apply_request, create_project, start, temp_dir};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use sha2::{Digest, Sha256};

fn hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sketch(directory: &std::path::Path, name: &str, width: u32, height: u32) -> Request {
    Request::CreateSketch(v1::CreateSketch { directory: directory.display().to_string(), name: name.into(), width, height })
}

async fn created(client: &mut Client, request: Request) -> Result<v1::SketchCreated, String> {
    match client.request(request).await.body {
        Body::SketchCreated(sketch) => Ok(sketch),
        Body::Error(error) => Err(error.message),
        other => panic!("expected SketchCreated, got {other:?}"),
    }
}

#[tokio::test]
async fn sketches_are_blank_pngs_in_a_projects_folder_never_over_anything() {
    let dir = temp_dir();
    let home = dir.join("home");
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let site = dir.join("site");
    std::fs::create_dir_all(site.join("drafts")).expect("mkdir");
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_project("Home", &home)).await;
    let site = std::fs::canonicalize(&site).expect("canonical");

    let made = created(&mut client, sketch(&site.join("drafts"), "Idea", 640, 480)).await.expect("made");
    assert_eq!(made.path, site.join("drafts/Idea.png").display().to_string());
    let bytes = std::fs::read(&made.path).expect("written");
    assert_eq!(made.sha256, hex(&bytes));
    assert_eq!((&bytes[..8], &bytes[12..16]), (b"\x89PNG\r\n\x1a\n".as_slice(), b"IHDR".as_slice()));
    assert_eq!((u32::from_be_bytes(bytes[16..20].try_into().unwrap()), u32::from_be_bytes(bytes[20..24].try_into().unwrap())), (640, 480));

    // Taken names get " 2", " 3"…, and nothing there is written over, a
    // link included.
    let second = created(&mut client, sketch(&site.join("drafts"), "Idea", 10, 10)).await.expect("made");
    assert_eq!(second.path, site.join("drafts/Idea 2.png").display().to_string());
    let target = dir.join("elsewhere.txt");
    std::fs::write(&target, "keep me").expect("write");
    crate::common::symlink(&target, &site.join("drafts/Idea 3.png")).expect("link");
    let fourth = created(&mut client, sketch(&site.join("drafts"), "Idea", 10, 10)).await.expect("made");
    assert_eq!(fourth.path, site.join("drafts/Idea 4.png").display().to_string());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep me");
    assert_eq!(std::fs::read(&made.path).unwrap(), bytes, "the first is as it was");
    // The largest is still small.
    let big = created(&mut client, sketch(&site, "Wall", 8192, 8192)).await.expect("made");
    assert!(std::fs::metadata(&big.path).unwrap().len() < 1024 * 1024);

    // Refused, each with a reason.
    let outside = dir.join("outside");
    std::fs::create_dir_all(&outside).expect("mkdir");
    std::fs::create_dir_all(home.join("annotations")).expect("mkdir");
    for (request, why) in [
        (sketch(&outside, "Idea", 10, 10), "one of your projects' folders"),
        (sketch(&home.join("annotations"), "Idea", 10, 10), "Claktory's own storage"),
        (sketch(&site.join("missing"), "Idea", 10, 10), "can't find the folder"),
        (sketch(std::path::Path::new("site"), "Idea", 10, 10), "site"),
        (sketch(&site, "", 10, 10), "can't be empty"),
        (sketch(&site, ".", 10, 10), "can't be empty"),
        (sketch(&site, "..", 10, 10), "can't be empty"),
        (sketch(&site, "../escape", 10, 10), "\"/\""),
        (sketch(&site, &"x".repeat(201), 10, 10), "at most 200"),
        (sketch(&site, "Idea", 0, 10), "1 to 8192"),
        (sketch(&site, "Idea", 10, 8193), "1 to 8192"),
    ] {
        let said = created(&mut client, request).await.expect_err("refused");
        assert!(said.contains(why), "{said}");
    }
    assert!(!outside.join("Idea.png").exists() && !home.join("annotations/Idea.png").exists() && !dir.join("escape.png").exists());

    // An ordinary image: ink goes on it as on any other.
    let blobs = home.join("annotations/blobs");
    std::fs::create_dir_all(&blobs).expect("mkdir");
    let mut preview = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    preview.extend_from_slice(&640u32.to_be_bytes());
    preview.extend_from_slice(&480u32.to_be_bytes());
    for blob in [b"strokes".as_slice(), preview.as_slice()] {
        std::fs::write(blobs.join(hex(blob)), blob).expect("blob");
    }
    let space = v1::InkSpace {
        kind: "image".into(),
        oriented_width: 640,
        oriented_height: 480,
        stored_width: 640,
        stored_height: 480,
        exif_orientation: 1,
        units: "px".into(),
        ..Default::default()
    };
    let save = v1::SaveInkLayer {
        path: made.path.clone(),
        source_sha256: made.sha256.clone(),
        space: Some(space),
        drawing_space: "blank_canvas".into(),
        drawing_to_document: vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        drawing_sha256: hex(b"strokes"),
        preview_sha256: hex(&preview),
        content_version: 1,
        client_save_id: "sketch-1".into(),
        ..Default::default()
    };
    match client.request(Request::SaveInkLayer(save)).await.body {
        Body::InkLayerSaved(saved) => assert_eq!(saved.revision, 1),
        other => panic!("expected InkLayerSaved, got {other:?}"),
    }
}
