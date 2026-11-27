//! Ink layers: saved with compare and set, a retry
//! answering what the save answered, durable before they're said to be
//! saved, kept per version of the file, deleted only at the revision seen,
//! and pruned with the blobs nothing refers to any more.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::common::{Client, start, temp_dir};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use sha2::{Digest, Sha256};

fn hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn png(width: u32, height: u32, salt: u8) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&width.to_be_bytes());
    png.extend_from_slice(&height.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0, salt]);
    png
}

/// Keeps `bytes` in the blob store as they'd be uploaded, `age` old.
fn put(home: &Path, bytes: &[u8], age: Duration) -> String {
    let sha = hex(bytes);
    let dir = home.join("annotations/blobs");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join(&sha);
    std::fs::write(&path, bytes).expect("write");
    std::fs::File::options().write(true).open(&path).and_then(|file| file.set_modified(SystemTime::now() - age)).expect("age");
    sha
}

fn age(home: &Path, sha: &str, age: Duration) {
    let path = home.join("annotations/blobs").join(sha);
    std::fs::File::options().write(true).open(path).and_then(|file| file.set_modified(SystemTime::now() - age)).expect("age");
}

fn kept(home: &Path, sha: &str) -> bool {
    home.join("annotations/blobs").join(sha).exists()
}

const DAYS_2: Duration = Duration::from_secs(2 * 86_400);

fn page(number: u32) -> v1::InkSpace {
    v1::InkSpace {
        kind: "pdf_page".into(),
        page_1based: number,
        r#box: "crop".into(),
        crop_box: vec![0.0, 0.0, 612.0, 792.0],
        media_box: vec![0.0, 0.0, 612.0, 792.0],
        rotation: 0,
        width: 612.0,
        height: 792.0,
        units: "pt".into(),
        ..v1::InkSpace::default()
    }
}

/// A save of page 3 of `path` (version `sha`), drawn as `drawing` with
/// `preview`.
fn save(id: &str, path: &Path, sha: &str, layer: &str, base: u32, drawing: &str, preview: &str) -> v1::SaveInkLayer {
    v1::SaveInkLayer {
        ink_layer_id: layer.into(),
        base_revision: base,
        path: path.display().to_string(),
        source_sha256: sha.into(),
        space: Some(page(3)),
        drawing_space: "page_overlay".into(),
        drawing_to_document: vec![1.0, 0.0, 0.0, -1.0, 0.0, 792.0],
        drawing_sha256: drawing.into(),
        preview_sha256: preview.into(),
        content_version: 1,
        client_save_id: id.into(),
        conflict_of: String::new(),
        os_version: "27.2".into(),
    }
}

async fn saved(client: &mut Client, request: v1::SaveInkLayer) -> Result<v1::InkLayerSaved, String> {
    match client.request(Request::SaveInkLayer(request)).await.body {
        Body::InkLayerSaved(saved) => Ok(saved),
        Body::Error(error) => Err(error.message),
        other => panic!("expected InkLayerSaved, got {other:?}"),
    }
}

async fn layers(client: &mut Client, path: &Path) -> v1::InkLayerList {
    let request = Request::ListInkLayers(v1::ListInkLayers { path: path.display().to_string() });
    match client.request(request).await.body {
        Body::InkLayerList(list) => list,
        other => panic!("expected InkLayerList, got {other:?}"),
    }
}

async fn delete(client: &mut Client, layer: &str, base: u32) -> Result<(), String> {
    let request = Request::DeleteInkLayer(v1::DeleteInkLayer { ink_layer_id: layer.into(), base_revision: base });
    match client.request(request).await.body {
        Body::Ack(_) => Ok(()),
        Body::Error(error) => Err(error.message),
        other => panic!("expected Ack, got {other:?}"),
    }
}

fn records(dir: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db")
}

#[tokio::test]
async fn layers_save_with_compare_and_set_and_a_retry_answers_the_same() {
    let dir = temp_dir();
    let home = dir.join("home");
    let (addr, _factory) = start(&dir).await;
    let (mut ipad, _, _) = Client::connect_as(addr, v1::DeviceKind::Tablet).await;
    let (mut mac, _, _) = Client::connect(addr).await;
    let file = std::fs::canonicalize(&dir).expect("canonical").join("paper.pdf");
    std::fs::write(&file, b"%PDF-1.7\nfirst version").expect("write");
    let first = hex(b"%PDF-1.7\nfirst version");
    let (d1, p1) = (put(&home, b"drawing one", Duration::ZERO), put(&home, &png(1224, 1584, 1), Duration::ZERO));
    let (d2, p2) = (put(&home, b"drawing two", Duration::ZERO), put(&home, &png(1224, 1584, 2), Duration::ZERO));

    let list = layers(&mut ipad, &file).await;
    assert_eq!((list.path.as_str(), list.current_sha256.as_str(), list.layers.len()), (file.to_str().unwrap(), first.as_str(), 0));

    // Made, durable: another reader of the records sees it the moment it's said.
    let made = saved(&mut ipad, save("s1", &file, &first, "", 0, &d1, &p1)).await.expect("saved");
    assert_eq!((made.client_save_id.as_str(), made.revision, made.conflict.is_none()), ("s1", 1, true));
    let layer = made.ink_layer_id.clone();
    assert!(layer.starts_with("layer-"), "{layer}");
    let rows: i64 = records(&dir).query_row("SELECT COUNT(*) FROM ink_revisions", [], |row| row.get(0)).expect("count");
    assert_eq!(rows, 1);
    // Every device hears of it.
    let heard = mac.wait_for(|body| if let Body::InkLayerList(list) = body { Some(list.clone()) } else { None }).await;
    assert_eq!(heard.layers.len(), 1);
    let shown = &heard.layers[0];
    assert_eq!((shown.id.as_str(), shown.revision, shown.parent_revision, shown.outdated), (layer.as_str(), 1, 0, false));
    assert_eq!((shown.source_mime.as_str(), shown.source_size), ("application/pdf", 22));
    assert_eq!((shown.device_kind.as_str(), shown.os_version.as_str(), shown.drawing_space.as_str()), ("tablet", "27.2", "page_overlay"));
    assert_eq!(shown.drawing_to_document, vec![1.0, 0.0, 0.0, -1.0, 0.0, 792.0]);
    assert_eq!(shown.space, Some(page(3)));
    let drawing = shown.drawing.clone().expect("drawing");
    assert_eq!(
        (drawing.format.as_str(), drawing.sha256.as_str(), drawing.bytes, drawing.content_version),
        ("pencilkit", d1.as_str(), 11, 1)
    );
    let preview = shown.preview.clone().expect("preview");
    assert_eq!((preview.sha256.as_str(), preview.pixel_width, preview.pixel_height), (p1.as_str(), 1224, 1584));

    // A retry answers what the save did, and makes nothing more.
    assert_eq!(saved(&mut ipad, save("s1", &file, &first, "", 0, &d2, &p2)).await, Ok(made.clone()));
    // Another device making the same surface's layer gets the one there is.
    let second = saved(&mut mac, save("m1", &file, &first, "", 0, &d2, &p2)).await.expect("answered");
    assert_eq!((second.revision, second.ink_layer_id.as_str()), (0, layer.as_str()));
    assert_eq!(second.conflict.as_ref().map(|conflict| conflict.revision), Some(1));

    // The next revision, then a save over the old one: nothing overwritten.
    let next = saved(&mut ipad, save("s2", &file, &first, &layer, 1, &d2, &p2)).await.expect("saved");
    assert_eq!((next.ink_layer_id.as_str(), next.revision), (layer.as_str(), 2));
    let stale = saved(&mut mac, save("m2", &file, &first, &layer, 1, &d1, &p1)).await.expect("answered");
    assert_eq!(stale.revision, 0);
    let latest = stale.conflict.expect("the layer as it is");
    assert_eq!((latest.revision, latest.parent_revision, latest.drawing.unwrap().sha256), (2, 1, d2.clone()));
    // Kept both: a layer of its own beside it.
    let mut beside = save("m3", &file, &first, "", 0, &d1, &p1);
    beside.conflict_of = layer.clone();
    let beside = saved(&mut mac, beside).await.expect("kept beside");
    assert_eq!(beside.revision, 1);
    assert_ne!(beside.ink_layer_id, layer);
    let list = layers(&mut mac, &file).await;
    assert_eq!(
        list.layers.iter().map(|layer| (layer.id.clone(), layer.conflict_of.clone())).collect::<Vec<_>>(),
        vec![(beside.ink_layer_id.clone(), layer.clone()), (layer.clone(), String::new())]
    );

    // Now its blobs can be read.
    let read = Request::ReadInkBlob(v1::ReadInkBlob { sha256: d2.clone(), offset: 0 });
    match mac.request(read).await.body {
        Body::InkBlobData(data) => assert_eq!((data.error.as_str(), data.data.as_slice()), ("", b"drawing two".as_slice())),
        other => panic!("expected InkBlobData, got {other:?}"),
    }

    // The file changes: its layers are outdated, ink begun on them still
    // saves, and a new layer needs the file as it is now.
    std::fs::write(&file, b"%PDF-1.7\nsecond version").expect("write");
    let now = hex(b"%PDF-1.7\nsecond version");
    let list = layers(&mut ipad, &file).await;
    assert_eq!(list.current_sha256, now);
    assert!(list.layers.iter().all(|layer| layer.outdated));
    let later = saved(&mut ipad, save("s3", &file, &first, &layer, 2, &d1, &p1)).await.expect("saved on the old version");
    assert_eq!(later.revision, 3);
    assert_eq!(
        saved(&mut ipad, save("s4", &file, &first, "", 0, &d1, &p1)).await.map(|saved| saved.revision),
        Err("This file changed since you opened it.".to_owned())
    );
    let fresh = saved(&mut ipad, save("s5", &file, &now, "", 0, &d1, &p1)).await.expect("a layer on the new version");
    assert_ne!(fresh.ink_layer_id, layer);
    let list = layers(&mut ipad, &file).await;
    assert_eq!(list.layers.len(), 3);
    assert!(!list.layers.iter().find(|shown| shown.id == fresh.ink_layer_id).unwrap().outdated);

    // Deleted only at the revision seen.
    assert_eq!(delete(&mut mac, &layer, 2).await, Err("This ink changed on another device. Look at it again, then delete it.".to_owned()));
    assert_eq!(delete(&mut mac, &layer, 3).await, Ok(()));
    let heard = ipad.wait_for(|body| if let Body::InkLayerList(list) = body { Some(list.clone()) } else { None }).await;
    assert!(heard.layers.iter().all(|shown| shown.id != layer));
    assert!(delete(&mut mac, &layer, 3).await.unwrap_err().contains("no ink layer"));
    assert_eq!(layers(&mut mac, &file).await.layers.len(), 2);
    assert!(saved(&mut ipad, save("s6", &file, &first, &layer, 3, &d1, &p1)).await.unwrap_err().contains("no ink layer"));
}

#[tokio::test]
async fn saves_are_refused_with_a_reason() {
    let dir = temp_dir();
    let home = dir.join("home");
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let file = dir.join("paper.pdf");
    std::fs::write(&file, b"%PDF-1.7\n").expect("write");
    let sha = hex(b"%PDF-1.7\n");
    let (drawing, preview) = (put(&home, b"drawing", Duration::ZERO), put(&home, &png(10, 10, 0), Duration::ZERO));
    let not_png = put(&home, b"GIF89a", Duration::ZERO);
    let big = put(&home, &vec![7u8; 8 * 1024 * 1024 + 1], Duration::ZERO);
    let base = || save("x", &file, &sha, "", 0, &drawing, &preview);

    let cases: Vec<(v1::SaveInkLayer, &str)> = vec![
        (v1::SaveInkLayer { client_save_id: String::new(), ..base() }, "client_save_id"),
        (v1::SaveInkLayer { drawing_sha256: "f".repeat(64), ..base() }, "upload it first"),
        (v1::SaveInkLayer { drawing_sha256: "../../config.json".into(), ..base() }, "upload it first"),
        (v1::SaveInkLayer { drawing_sha256: big.clone(), ..base() }, "at most 8 MiB"),
        (v1::SaveInkLayer { preview_sha256: not_png, ..base() }, "isn't a PNG"),
        (v1::SaveInkLayer { preview_sha256: big, ..base() }, "over 4 MiB"),
        (v1::SaveInkLayer { space: Some(page(0)), ..base() }, "page is 1 to 2000"),
        (v1::SaveInkLayer { space: Some(v1::InkSpace { rotation: 45, ..page(3) }), ..base() }, "rotation"),
        (v1::SaveInkLayer { space: None, ..base() }, "surface"),
        (v1::SaveInkLayer { drawing_space: "oriented_image".into(), ..base() }, "doesn't fit"),
        (v1::SaveInkLayer { drawing_to_document: vec![1.0, 0.0, 0.0], ..base() }, "six numbers"),
        (v1::SaveInkLayer { drawing_to_document: vec![1.0, 0.0, 0.0, 1.0, f64::NAN, 0.0], ..base() }, "six numbers"),
        (v1::SaveInkLayer { content_version: 0, ..base() }, "content_version"),
        (v1::SaveInkLayer { base_revision: 2, ..base() }, "no revision to save over"),
        (v1::SaveInkLayer { path: dir.join("gone.pdf").display().to_string(), ..base() }, "can't find"),
        (v1::SaveInkLayer { path: "paper.pdf".into(), ..base() }, "paper.pdf"),
        (v1::SaveInkLayer { source_sha256: "0".repeat(64), ..base() }, "changed since you opened it"),
        (v1::SaveInkLayer { ink_layer_id: "layer-99".into(), base_revision: 1, ..base() }, "no ink layer"),
        (v1::SaveInkLayer { conflict_of: "layer-99".into(), ..base() }, "isn't a layer on this surface"),
    ];
    for (request, why) in cases {
        let said = saved(&mut client, request.clone()).await.expect_err("refused");
        assert!(said.contains(why), "{request:?}: {said}");
    }
    assert!(layers(&mut client, &file).await.layers.is_empty());

    // A revision is for its own layer's file and surface.
    let made = saved(&mut client, base()).await.expect("saved");
    let mut other_page = save("y", &file, &sha, &made.ink_layer_id, 1, &drawing, &preview);
    other_page.space = Some(page(4));
    assert!(saved(&mut client, other_page).await.unwrap_err().contains("isn't for that layer's file and surface"));
}

#[tokio::test]
async fn old_revisions_and_the_blobs_nothing_refers_to_are_pruned() {
    let dir = temp_dir();
    let home = dir.join("home");
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let file = dir.join("photo.pdf");
    std::fs::write(&file, b"%PDF-1.7\n").expect("write");
    let sha = hex(b"%PDF-1.7\n");
    // Uploaded long ago and never saved, and just now (its save is coming).
    let orphan = put(&home, b"abandoned", DAYS_2);
    let coming = put(&home, b"on its way", Duration::ZERO);
    // Revision 1's drawing was uploaded long ago too, and it stays: a revision names it.
    let (d1, p1) = (put(&home, b"one", DAYS_2), put(&home, &png(10, 10, 1), Duration::ZERO));
    let (d2, p2) = (put(&home, b"two", Duration::ZERO), put(&home, &png(10, 10, 2), Duration::ZERO));
    let d3 = put(&home, b"three", Duration::ZERO);

    let made = saved(&mut client, save("a", &file, &sha, "", 0, &d1, &p1)).await.expect("saved");
    let layer = made.ink_layer_id;
    // The first save sweeps the store, in the background.
    for _ in 0..100 {
        if !kept(&home, &orphan) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!kept(&home, &orphan), "a blob nothing refers to goes once it's a day old");
    assert!(kept(&home, &coming) && kept(&home, &d1) && kept(&home, &p1));

    saved(&mut client, save("b", &file, &sha, &layer, 1, &d2, &p2)).await.expect("saved");
    // A day on: revision 1 was saved over long ago, and its blobs are old.
    let ago = |days: i64| SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_millis() as i64 - days * 86_400_000;
    let db = records(&dir);
    db.execute("UPDATE ink_revisions SET superseded = ?1 WHERE revision = 1", [ago(2)]).expect("aged");
    db.execute("UPDATE ink_saves SET created = ?1 WHERE client_save_id = 'a'", [ago(8)]).expect("aged");
    age(&home, &d1, DAYS_2);
    age(&home, &p1, DAYS_2);
    age(&home, &d2, DAYS_2);
    // Revision 3 has revision 1's preview again.
    saved(&mut client, save("c", &file, &sha, &layer, 2, &d3, &p1)).await.expect("saved");
    let revisions: Vec<u32> = {
        let mut statement = db.prepare("SELECT revision FROM ink_revisions ORDER BY revision").unwrap();
        statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    };
    assert_eq!(revisions, vec![2, 3], "revision 2 was saved over just now, so it stays a day");
    assert!(!kept(&home, &d1), "revision 1's drawing went with it");
    assert!(kept(&home, &p1), "its preview is revision 3's too");
    assert!(kept(&home, &d2) && kept(&home, &p2) && kept(&home, &d3) && kept(&home, &coming));
    let saves: Vec<String> = {
        let mut statement = db.prepare("SELECT client_save_id FROM ink_saves ORDER BY client_save_id").unwrap();
        statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    };
    assert_eq!(saves, vec!["b", "c"], "what saves answered is kept a week");

    // A deleted layer's blobs go when they're old, and are left to the
    // sweep while they're young.
    delete(&mut client, &layer, 3).await.expect("deleted");
    assert!(!kept(&home, &p1) && !kept(&home, &d2));
    assert!(kept(&home, &d3));
    let rows: i64 = db.query_row("SELECT COUNT(*) FROM ink_revisions", [], |row| row.get(0)).unwrap();
    assert_eq!(rows, 0);
}
