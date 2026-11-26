//! Apps on other machines fetch their release's app from Factory (FetchApp),
//! chunk by chunk, to update themselves.

use std::io::Write as _;

use crate::common::{Client, View, apply_request, start, temp_dir};
use factory_protocol::v1::{self, ErrorCode, client_message, server_message::Body};
use sha2::{Digest, Sha256};

fn fetch(version: &str, offset: u64) -> client_message::Body {
    client_message::Body::FetchApp(v1::FetchApp { version: version.into(), offset, ..Default::default() })
}

/// An app bundle with a file that doesn't compress, so it takes chunks.
fn release(dir: &std::path::Path, version: &str) -> Vec<u8> {
    let app = dir.join("home").join("releases").join(version).join("Factory.app").join("Contents");
    std::fs::create_dir_all(app.join("MacOS")).expect("mkdir");
    std::fs::write(app.join("Info.plist"), "<plist/>").expect("write");
    let mut noise = Vec::with_capacity(2_500_000);
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    while noise.len() < 2_500_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        noise.extend_from_slice(&state.to_le_bytes());
    }
    std::fs::File::create(app.join("MacOS").join("Factory")).and_then(|mut file| file.write_all(&noise)).expect("write");
    std::fs::write(dir.join("home").join("releases").join(version).join("factoryd"), "backend").expect("write");
    noise
}

/// Fetches the whole archive, waiting while it's prepared; Err with the
/// refusal's message.
async fn fetch_all(client: &mut Client, view: &mut View, version: &str) -> Result<(Vec<u8>, v1::AppChunk), String> {
    let mut archive = Vec::new();
    for _ in 0..400 {
        match apply_request(client, view, fetch(version, archive.len() as u64)).await {
            Body::AppChunk(chunk) if chunk.preparing => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            Body::AppChunk(chunk) => {
                assert_eq!(chunk.offset, archive.len() as u64);
                archive.extend_from_slice(&chunk.data);
                if archive.len() as u64 == chunk.size {
                    return Ok((archive, chunk));
                }
            }
            Body::Error(error) => return Err(error.message),
            other => panic!("{other:?}"),
        }
    }
    panic!("never ready")
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A Linux release has no app: it's the published Mac release's, downloaded
/// when an app first asks and served as it is.
#[tokio::test]
async fn a_release_without_an_app_serves_the_mac_releases() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    // The published Mac release: the app, factoryd and the rest.
    let published = dir.join("published");
    std::fs::create_dir_all(published.join("Factory.app/Contents")).expect("mkdir");
    std::fs::write(published.join("Factory.app/Contents/Info.plist"), "<plist/>").expect("write");
    std::fs::write(published.join("factoryd"), "mac backend").expect("write");
    let tarball = dir.join("factory-mac.tar.gz");
    let status = std::process::Command::new("tar")
        .args(["-czf"])
        .arg(&tarball)
        .arg("-C")
        .arg(&published)
        .args(["factoryd", "Factory.app"])
        .status()
        .expect("tar");
    assert!(status.success());
    let bytes = std::fs::read(&tarball).expect("read");
    let note = |release: &str, sha256: &str| {
        let folder = dir.join("home/releases").join(release);
        std::fs::create_dir_all(&folder).expect("mkdir");
        std::fs::write(folder.join("factoryd"), "linux backend").expect("write");
        let source = serde_json::json!({"url": format!("file://{}", tarball.display()), "sha256": sha256, "size": bytes.len()});
        std::fs::write(folder.join("app-source.json"), source.to_string()).expect("write");
    };
    note("2026.11.20-1012", &sha256(&bytes));
    let (archive, last) = fetch_all(&mut client, &mut view, "2026.11.20-1012").await.expect("the app");
    assert_eq!(archive, bytes);
    assert_eq!((last.sha256, last.app_name.as_str()), (sha256(&bytes), "Factory.app"));

    // A Mac release that isn't what the manifest said is refused, and said
    // again to every ask for a while rather than downloaded again at each
    // (friend bug #56): even once it would be right.
    note("2026.11.21-0900", &"0".repeat(64));
    let refused = fetch_all(&mut client, &mut view, "2026.11.21-0900").await.expect_err("refused");
    assert!(refused.contains("the Mac app it downloaded didn't match the release, so it wasn't used"), "{refused}");
    assert!(!refused.contains("SHA-256") && !refused.contains(&"0".repeat(64)), "no hashes for the person: {refused}");
    assert!(!dir.join("home/releases/2026.11.21-0900/app.tar.gz").exists());
    note("2026.11.21-0900", &sha256(&bytes));
    for _ in 0..3 {
        let again = apply_request(&mut client, &mut view, fetch("2026.11.21-0900", 0)).await;
        assert!(matches!(&again, Body::Error(error) if error.message == refused), "{again:?}");
    }
    assert!(!dir.join("home/releases/2026.11.21-0900/app.tar.gz").exists());
}

/// The Mac release is a .tar.xz since #454: a Linux Claktory reads its app's
/// name in Rust, with no xz on the computer, and serves it as it is, which
/// every Mac app's `tar -xzf` unpacks (friend bug #56).
#[tokio::test]
async fn a_mac_release_in_xz_is_served_too() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    // As publish.sh packs it: the app first, then the rest.
    let published = dir.join("published");
    std::fs::create_dir_all(published.join("Claktory.app/Contents")).expect("mkdir");
    std::fs::write(published.join("Claktory.app/Contents/Info.plist"), "<plist/>").expect("write");
    std::fs::write(published.join("factoryd"), "mac backend").expect("write");
    let plain = dir.join("factory-mac.tar");
    let status = std::process::Command::new("tar")
        .arg("-cf")
        .arg(&plain)
        .arg("-C")
        .arg(&published)
        .args(["Claktory.app", "factoryd"])
        .status()
        .expect("tar");
    assert!(status.success());
    // With xz as publish.sh runs it where there is one; else in Rust.
    let tarball = dir.join("factory-mac.tar.xz");
    let by_xz = std::process::Command::new("xz")
        .args(["-T1", "--lzma2=preset=9e,dict=192MiB", "-c"])
        .arg(&plain)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout);
    let bytes = by_xz.unwrap_or_else(|| {
        let mut packed = Vec::new();
        lzma_rs::xz_compress(&mut std::io::BufReader::new(std::fs::File::open(&plain).expect("tar")), &mut packed).expect("xz");
        packed
    });
    assert_eq!(&bytes[..6], b"\xFD7zXZ\0");
    std::fs::write(&tarball, &bytes).expect("write");
    let folder = dir.join("home/releases/0.1.24");
    std::fs::create_dir_all(&folder).expect("mkdir");
    std::fs::write(folder.join("factoryd"), "linux backend").expect("write");
    let source = serde_json::json!({"url": format!("file://{}", tarball.display()), "sha256": sha256(&bytes), "size": bytes.len()});
    std::fs::write(folder.join("app-source.json"), source.to_string()).expect("write");

    let (archive, last) = fetch_all(&mut client, &mut view, "0.1.24").await.expect("the app");
    assert_eq!(archive, bytes, "served as it is");
    assert_eq!(last.app_name, "Claktory.app");
    // And it unpacks as an app does it, where that tar is.
    if cfg!(target_os = "macos") {
        let fetched = dir.join("app.tar.gz.part");
        std::fs::write(&fetched, &archive).expect("write");
        let into = dir.join("unpacked");
        std::fs::create_dir_all(&into).expect("mkdir");
        let status =
            std::process::Command::new("/usr/bin/tar").arg("-xzf").arg(&fetched).arg("-C").arg(&into).arg("Claktory.app").status().expect("tar");
        assert!(status.success());
        assert_eq!(std::fs::read_to_string(into.join("Claktory.app/Contents/Info.plist")).expect("unpacked"), "<plist/>");
    }
}

#[tokio::test]
async fn an_app_on_another_machine_fetches_its_release_in_chunks() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let noise = release(&dir, "2026.11.20-1012");

    // The archive is made the first time it's asked for.
    let first = apply_request(&mut client, &mut view, fetch("2026.11.20-1012", 0)).await;
    assert!(matches!(&first, Body::AppChunk(chunk) if chunk.preparing && chunk.data.is_empty()), "{first:?}");
    let (archive, last) = fetch_all(&mut client, &mut view, "2026.11.20-1012").await.expect("the app");
    assert!(archive.len() > 2 << 20, "several chunks: {}", archive.len());
    assert_eq!((last.sha256, last.app_name.as_str()), (sha256(&archive), "Factory.app"));

    // It unpacks to the app, and only the app.
    let unpacked = dir.join("unpacked");
    std::fs::create_dir_all(&unpacked).expect("mkdir");
    std::fs::write(dir.join("app.tar.gz"), &archive).expect("write");
    let status = std::process::Command::new("tar").arg("-xzf").arg(dir.join("app.tar.gz")).arg("-C").arg(&unpacked).status().expect("tar");
    assert!(status.success());
    assert_eq!(std::fs::read(unpacked.join("Factory.app/Contents/MacOS/Factory")).expect("read"), noise);
    let top: Vec<String> =
        std::fs::read_dir(&unpacked).expect("ls").map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(top, ["Factory.app"]);

    // Smaller pieces for a slow link (#408): as asked, never over 1 MiB,
    // and the same bytes.
    let piece = |offset: u64, max_bytes: u64| {
        client_message::Body::FetchApp(v1::FetchApp { version: "2026.11.20-1012".into(), offset, max_bytes })
    };
    for (offset, max_bytes, expected) in [
        (0, 64 * 1024, 64 * 1024),
        (1000, 1, 1),
        (0, 4 << 20, 1 << 20),
        (archive.len() as u64 - 10, 64 * 1024, 10),
    ] {
        match apply_request(&mut client, &mut view, piece(offset, max_bytes)).await {
            Body::AppChunk(chunk) => {
                let at = usize::try_from(offset).expect("offset");
                assert_eq!((chunk.offset, chunk.data.len()), (offset, expected), "{max_bytes} from {offset}");
                assert_eq!(chunk.data, archive[at..at + expected]);
            }
            other => panic!("{other:?}"),
        }
    }

    // The end, and past it.
    let end = apply_request(&mut client, &mut view, fetch("2026.11.20-1012", archive.len() as u64)).await;
    assert!(matches!(&end, Body::AppChunk(chunk) if chunk.data.is_empty() && !chunk.preparing), "{end:?}");
    let past = apply_request(&mut client, &mut view, fetch("2026.11.20-1012", archive.len() as u64 + 1)).await;
    assert!(matches!(&past, Body::Error(error) if error.message.contains("past the archive's end")), "{past:?}");

    // Releases it doesn't have, or that have no app, are refused with why.
    std::fs::create_dir_all(dir.join("home/releases/2026.11.21-0900")).expect("mkdir");
    for (version, why) in [
        ("2026.11.19-0000", "this Claktory has no such release"),
        ("../releases/2026.11.20-1012", "this Claktory has no such release"),
        ("2026.11.21-0900", "the release has no app in it"),
    ] {
        match apply_request(&mut client, &mut view, fetch(version, 0)).await {
            Body::Error(error) => {
                assert_eq!(error.code(), ErrorCode::NotFound);
                assert!(error.message.ends_with(why), "{version}: {}", error.message);
            }
            other => panic!("{version}: {other:?}"),
        }
    }
}
