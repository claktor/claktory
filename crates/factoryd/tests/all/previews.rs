//! Phone-sized previews (#379): an app on a small screen asks ReadFile for a
//! preview sized to it and gets a smaller WebP that says it's a preview,
//! with the original's version, while a plain ReadFile still gets the
//! original.

use std::path::Path;

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, FilePreview, client_message, server_message::Body};
use image::ImageEncoder;

const WIDTH: u32 = 1320;
const HEIGHT: u32 = 2868;

fn read(path: &Path, preview: bool) -> client_message::Body {
    let (preview_width, preview_height) = if preview { (WIDTH, HEIGHT) } else { (0, 0) };
    client_message::Body::ReadFile(v1::ReadFile { path: path.display().to_string(), preview_width, preview_height, ..Default::default() })
}

fn content(body: Body) -> v1::FileContent {
    match body {
        Body::FileContent(content) => content,
        other => panic!("expected file content, got {other:?}"),
    }
}

fn sha(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A screenshot-like picture: bands of colour and fine noise, so it doesn't
/// compress to nothing.
fn picture(width: u32, height: u32, seed: u32) -> image::RgbImage {
    let mut state = seed.wrapping_mul(2_654_435_761).max(1);
    image::RgbImage::from_fn(width, height, |x, y| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        let noise = (state & 0x1f) as u8;
        image::Rgb([(x / 7 % 200) as u8 + noise, (y / 11 % 200) as u8 + noise, ((x + y) / 13 % 200) as u8])
    })
}

fn png(width: u32, height: u32, seed: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let picture = picture(width, height, seed);
    image::codecs::png::PngEncoder::new(&mut bytes).write_image(&picture, width, height, image::ExtendedColorType::Rgb8).expect("png");
    bytes
}

/// A JPEG stored 3000 wide and 2000 high that shows turned a quarter (EXIF
/// orientation 6), so 2000 wide and 3000 high.
fn turned_jpeg() -> Vec<u8> {
    let (width, height) = (3000, 2000);
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 95);
    // TIFF, little-endian: one entry, Orientation (0x0112), a SHORT, 6.
    let exif =
        [b"II*\0".as_slice(), &8u32.to_le_bytes(), &1u16.to_le_bytes(), &[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0], &0u32.to_le_bytes()]
            .concat();
    encoder.set_exif_metadata(exif).expect("exif");
    encoder.write_image(&picture(width, height, 7), width, height, image::ExtendedColorType::Rgb8).expect("jpeg");
    bytes
}

fn decoded(webp: &[u8]) -> image::DynamicImage {
    image::load_from_memory_with_format(webp, image::ImageFormat::WebP).expect("a WebP")
}

#[tokio::test]
async fn a_phone_gets_a_smaller_preview_that_says_so_and_the_original_stays_as_it_was() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let shot = dir.join("shot.png");
    let original = png(2064, 2752, 1);
    std::fs::write(&shot, &original).expect("write");

    let preview = content(client.request(read(&shot, true)).await.body);
    assert_eq!(preview.error, "");
    assert_eq!(preview.preview(), FilePreview::Made);
    assert_eq!((preview.mime.as_str(), preview.truncated, preview.offset), ("image/webp", false, 0));
    assert_eq!(preview.size, preview.data.len() as u64);
    assert!(preview.size * 3 < original.len() as u64, "{} of {}", preview.size, original.len());
    assert_eq!((preview.pixel_width, preview.pixel_height), (1320, 1760), "fits the screen's width");
    assert_eq!((preview.original_pixel_width, preview.original_pixel_height), (2064, 2752));
    assert_eq!(preview.original_size, original.len() as u64);
    assert_eq!(preview.sha256, sha(&original), "the original's version");
    let shown = decoded(&preview.data);
    assert_eq!((shown.width(), shown.height()), (1320, 1760));

    // A plain ReadFile is the original, byte for byte.
    let whole = content(client.request(read(&shot, false)).await.body);
    assert_eq!(whole.preview(), FilePreview::None);
    assert_eq!((whole.mime.as_str(), whole.data == original, whole.sha256 == sha(&original)), ("image/png", true, true));
    assert_eq!((whole.pixel_width, whole.original_size), (0, 0));

    // Kept in the data folder's cache, by the original's hash and the
    // screen, and read from there the next time.
    let cache = factory.home().root().join("cache/previews");
    let kept = cache.join(format!("{}-1320x2868.preview", sha(&original)));
    assert!(kept.is_file(), "{:?}", std::fs::read_dir(&cache).map(|dir| dir.flatten().map(|e| e.file_name()).collect::<Vec<_>>()));
    let again = content(client.request(read(&shot, true)).await.body);
    assert_eq!(again.data, preview.data);

    // Read a piece at a time, it's the preview's bytes that are paced.
    let piece = |offset: u64| {
        client_message::Body::ReadFile(v1::ReadFile {
            path: shot.display().to_string(),
            preview_width: WIDTH,
            preview_height: HEIGHT,
            offset,
            max_bytes: 1000,
            expected_sha256: sha(&original),
            ..Default::default()
        })
    };
    let first = content(client.request(piece(0)).await.body);
    let second = content(client.request(piece(1000)).await.body);
    assert_eq!((first.truncated, first.offset, first.size), (true, 0, preview.size));
    assert_eq!([first.data.as_slice(), &second.data].concat(), preview.data[..2000]);
    assert_eq!(second.preview(), FilePreview::Made);

    // A changed original: the old version is "changed", and the new one
    // gets a preview of its own.
    let changed = png(2064, 2752, 2);
    std::fs::write(&shot, &changed).expect("write");
    let stale = content(client.request(piece(1000)).await.body);
    assert_eq!((stale.error.as_str(), stale.data.len()), ("changed", 0));
    let fresh = content(client.request(read(&shot, true)).await.body);
    assert_eq!((fresh.preview(), fresh.sha256 == sha(&changed)), (FilePreview::Made, true));
    assert_ne!(fresh.data, preview.data);
}

#[tokio::test]
async fn what_needs_no_preview_comes_as_the_original() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    // Small and within the screen.
    let small = dir.join("small.png");
    let bytes = png(300, 200, 3);
    std::fs::write(&small, &bytes).expect("write");
    // Not an image.
    let notes = dir.join("notes.md");
    std::fs::write(&notes, "# Notes\n").expect("write");
    // An animated GIF stays one.
    let gif = dir.join("moving.gif");
    let mut moving = b"GIF89a".to_vec();
    moving.extend(vec![0; 300_000]);
    std::fs::write(&gif, &moving).expect("write");

    for (path, original) in [(&small, bytes), (&notes, b"# Notes\n".to_vec())] {
        let answer = content(client.request(read(path, true)).await.body);
        assert_eq!(
            (answer.preview(), answer.data == original, answer.sha256 == sha(&original)),
            (FilePreview::None, true, true),
            "{path:?}"
        );
    }
    let answer = content(client.request(read(&gif, true)).await.body);
    assert_eq!((answer.preview(), answer.mime.as_str()), (FilePreview::None, "image/gif"));
    let gone = content(client.request(read(&dir.join("gone.png"), true)).await.body);
    assert_eq!(gone.error, format!("No such file: {}", dir.join("gone.png").display()));
}

#[tokio::test]
async fn a_photo_shows_turned_as_its_camera_said() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let photo = dir.join("photo.jpg");
    std::fs::write(&photo, turned_jpeg()).expect("write");

    let preview = content(client.request(read(&photo, true)).await.body);
    assert_eq!(preview.preview(), FilePreview::Made, "{}", preview.error);
    assert_eq!((preview.original_pixel_width, preview.original_pixel_height), (2000, 3000), "as it shows, turned");
    assert_eq!((preview.pixel_width, preview.pixel_height), (1320, 1980));
    let shown = decoded(&preview.data);
    assert_eq!((shown.width(), shown.height()), (1320, 1980));
}

#[tokio::test]
async fn showing_an_image_makes_its_preview_for_the_screens_that_asked() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let site = dir.join("site");
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    // A phone asked for a preview once: its screen is known.
    let earlier = site.join("earlier.png");
    std::fs::write(&earlier, png(2000, 3000, 4)).expect("write");
    assert_eq!(content(client.request(read(&earlier, true)).await.body).preview(), FilePreview::Made);

    let render = png(2400, 1800, 5);
    std::fs::write(site.join("render.png"), &render).expect("write");
    apply_request(&mut client, &mut view, send_message("builder", r#"call show {"path":"render.png"}"#)).await;
    view.follow(&mut client, |view| view.agent("builder").status() == AgentStatus::Idle).await;
    let kept = factory.home().root().join(format!("cache/previews/{}-1320x2868.preview", sha(&render)));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !kept.is_file() {
        assert!(std::time::Instant::now() < deadline, "no preview made ahead");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// A picture sent from a device has its previews made as its upload
/// finishes, for the screens that asked lately: another device's first look
/// finds them made.
#[tokio::test]
async fn an_uploaded_picture_has_its_previews_made_ahead() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let site = dir.join("site");
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let earlier = site.join("earlier.png");
    std::fs::write(&earlier, png(2000, 3000, 6)).expect("write");
    assert_eq!(content(client.request(read(&earlier, true)).await.body).preview(), FilePreview::Made);

    let photo = png(2400, 1800, 7);
    let sent = crate::uploads::upload(&mut client, "u1", "builder", "photo.png", &photo, "image/png").await;
    let kept = factory.home().root().join(format!("cache/previews/{}-1320x2868.preview", sha(&photo)));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !kept.is_file() {
        assert!(std::time::Instant::now() < deadline, "no preview made ahead");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    // And it's the one a device asking then gets.
    let asked = content(client.request(read(Path::new(&sent.path), true)).await.body);
    assert_eq!(asked.preview(), FilePreview::Made);
}
