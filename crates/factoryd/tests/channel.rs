mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Client, home, start, temp_dir};
use factory_protocol::v1::server_message::Body;
use factoryd::channel::{Channel, ChannelError, Checked};
use sha2::{Digest, Sha256};

fn run(command: &mut Command) {
    let status = command.status().expect("run");
    assert!(status.success(), "{command:?}");
}

/// tar, wherever this machine has it.
fn tar() -> Command {
    Command::new(if cfg!(target_os = "macos") { "/usr/bin/tar" } else { "tar" })
}

/// A release as `scripts/publish.sh` packs it: a signed app (on a Mac; a
/// Linux release has none), a backend and its version, in a tarball.
fn tarball(dir: &Path, version: &str) -> PathBuf {
    let content = dir.join(format!("content-{version}"));
    std::fs::create_dir_all(&content).expect("content");
    std::fs::write(content.join("factoryd"), format!("backend {version}")).expect("factoryd");
    std::fs::write(content.join("backend-version"), format!("{version}\n")).expect("backend-version");
    let tarball = dir.join(format!("factory-{version}.tar.gz"));
    if !cfg!(target_os = "macos") {
        run(tar().arg("-czf").arg(&tarball).arg("-C").arg(&content).args(["factoryd", "backend-version"]));
        return tarball;
    }
    let app = content.join("Factory.app/Contents");
    std::fs::create_dir_all(app.join("MacOS")).expect("app");
    std::fs::write(app.join("MacOS/factory"), "#!/bin/sh\n").expect("app binary");
    std::fs::write(
        app.join("Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>CFBundleExecutable</key><string>factory</string><key>CFBundleIdentifier</key><string>dev.factory.test</string></dict></plist>
"#,
    )
    .expect("plist");
    run(Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(content.join("Factory.app")));
    run(tar().env("COPYFILE_DISABLE", "1").arg("-czf").arg(&tarball).arg("-C").arg(&content).args([
        "Factory.app",
        "factoryd",
        "backend-version",
    ]));
    tarball
}

/// Publishes `tarball` as `version` in a channel in `dir`, for this machine
/// (as the Mac release, and as the Linux one for this architecture), and
/// returns the manifest's address.
fn publish(dir: &Path, version: &str, tarball: &Path, sha256: Option<&str>) -> String {
    let bytes = std::fs::read(tarball).expect("tarball");
    let hash: String = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    let here = serde_json::json!({
        "url": format!("file://{}", tarball.display()),
        "sha256": sha256.unwrap_or(&hash),
        "size": bytes.len(),
    });
    let mut manifest = here.clone();
    manifest["version"] = version.into();
    manifest["min_macos"] = "12.0".into();
    manifest["archs"] = serde_json::json!(["arm64", "x86_64"]);
    manifest["linux"] = serde_json::json!({ std::env::consts::ARCH: here });
    write_manifest(dir, &manifest)
}

fn write_manifest(dir: &Path, manifest: &serde_json::Value) -> String {
    let path = dir.join("latest.json");
    std::fs::write(&path, manifest.to_string()).expect("manifest");
    format!("file://{}", path.display())
}

fn latest(dir: &Path) -> String {
    std::fs::read_to_string(home(dir).releases().join("latest")).unwrap_or_default().trim().to_owned()
}

fn release_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(home(dir).releases())
        .map(|entries| entries.map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

#[tokio::test]
async fn a_newer_published_release_is_downloaded_and_offered() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.latest_release, "");

    let url = publish(&dir, "2026.11.20-1012", &tarball(&dir, "2026.11.20-1012"), None);
    let mut channel = Channel::new(home(&dir), url);
    assert_eq!(channel.check().await.expect("check"), Checked::Downloaded("2026.11.20-1012".into()));
    let release = home(&dir).releases().join("2026.11.20-1012");
    assert_eq!(std::fs::read_to_string(release.join("factoryd")).expect("factoryd"), "backend 2026.11.20-1012");
    assert_eq!(release.join("Factory.app/Contents/Info.plist").is_file(), cfg!(target_os = "macos"));
    assert_eq!(latest(&dir), "2026.11.20-1012");
    assert_eq!(release_files(&dir), ["2026.11.20-1012", "latest"]);
    let offered = client
        .wait_for(|body| match body {
            Body::Workspace(workspace) if !workspace.latest_release.is_empty() => Some(workspace.latest_release.clone()),
            _ => None,
        })
        .await;
    assert_eq!(offered, "2026.11.20-1012");

    // Nothing newer: nothing to do.
    assert_eq!(channel.check().await.expect("check"), Checked::UpToDate);
}

#[tokio::test]
async fn an_older_release_than_the_one_here_is_left_alone() {
    let dir = temp_dir();
    let releases = home(&dir).releases();
    std::fs::create_dir_all(releases.join("2026.11.21-0900")).expect("release");
    std::fs::write(releases.join("2026.11.21-0900/factoryd"), "local").expect("factoryd");
    std::fs::write(releases.join("latest"), "2026.11.21-0900\n").expect("latest");
    let url = publish(&dir, "2026.11.20-1012", &tarball(&dir, "2026.11.20-1012"), None);
    assert_eq!(Channel::new(home(&dir), url).check().await.expect("check"), Checked::UpToDate);
    assert_eq!(latest(&dir), "2026.11.21-0900");
}

#[tokio::test]
async fn a_release_that_doesnt_match_its_manifest_is_refused_once_and_leaves_nothing() {
    let dir = temp_dir();
    let url = publish(&dir, "2026.11.20-1012", &tarball(&dir, "2026.11.20-1012"), Some(&"0".repeat(64)));
    let mut channel = Channel::new(home(&dir), url);
    let error = channel.check().await.expect_err("a wrong hash");
    assert!(matches!(error, ChannelError::Rejected { .. }), "{error}");
    assert_eq!(latest(&dir), "");
    assert_eq!(release_files(&dir), Vec::<String>::new());
    // Not downloaded again.
    assert_eq!(channel.check().await.expect("check"), Checked::Skipped("2026.11.20-1012".into()));
}

#[tokio::test]
async fn a_release_without_a_backend_is_refused() {
    let dir = temp_dir();
    let content = dir.join("broken");
    std::fs::create_dir_all(content.join("Factory.app")).expect("app");
    let tarball = dir.join("broken.tar.gz");
    run(tar().arg("-czf").arg(&tarball).arg("-C").arg(&content).arg("Factory.app"));
    let url = publish(&dir, "2026.11.20-1012", &tarball, None);
    let error = Channel::new(home(&dir), url).check().await.expect_err("no factoryd");
    assert!(error.to_string().contains("no factoryd"), "{error}");
    assert_eq!(latest(&dir), "");
}

/// A Factory on a date-stamped release (as every one was before v0.1.0) gets
/// the first numbered one, and numbered releases keep their order past 9.
#[tokio::test]
async fn numbered_releases_are_newer_than_date_stamps_and_ordered_by_number() {
    let dir = temp_dir();
    let releases = home(&dir).releases();
    for old in ["2026.11.19-1958", "v0.1.8", "v0.1.9"] {
        std::fs::create_dir_all(releases.join(old)).expect("release");
        std::fs::write(releases.join(old).join("factoryd"), "old").expect("factoryd");
    }
    std::fs::write(releases.join("latest"), "v0.1.9\n").expect("latest");
    let url = publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    assert_eq!(Channel::new(home(&dir), url).check().await.expect("check"), Checked::Downloaded("v0.1.10".into()));
    // The oldest goes: the date stamp, not v0.1.10 ("v0.1.10" < "v0.1.9" as text).
    assert_eq!(release_files(&dir), ["latest", "v0.1.10", "v0.1.8", "v0.1.9"]);

    let dir = temp_dir();
    let releases = home(&dir).releases();
    std::fs::create_dir_all(releases.join("2026.11.19-1958")).expect("release");
    std::fs::write(releases.join("2026.11.19-1958/factoryd"), "old").expect("factoryd");
    std::fs::write(releases.join("latest"), "2026.11.19-1958\n").expect("latest");
    let url = publish(&dir, "v0.1.0", &tarball(&dir, "v0.1.0"), None);
    assert_eq!(Channel::new(home(&dir), url).check().await.expect("check"), Checked::Downloaded("v0.1.0".into()));
    assert_eq!(latest(&dir), "v0.1.0");
}

#[tokio::test]
async fn only_the_newest_three_releases_are_kept() {
    let dir = temp_dir();
    let releases = home(&dir).releases();
    for old in ["2026.11.17-0900", "2026.11.18-0900", "2026.11.19-0900"] {
        std::fs::create_dir_all(releases.join(old)).expect("release");
        std::fs::write(releases.join(old).join("factoryd"), "old").expect("factoryd");
    }
    std::fs::write(releases.join("latest"), "2026.11.19-0900\n").expect("latest");
    let url = publish(&dir, "2026.11.20-1012", &tarball(&dir, "2026.11.20-1012"), None);
    assert_eq!(Channel::new(home(&dir), url).check().await.expect("check"), Checked::Downloaded("2026.11.20-1012".into()));
    assert_eq!(release_files(&dir), ["2026.11.18-0900", "2026.11.19-0900", "2026.11.20-1012", "latest"]);
}

#[tokio::test]
async fn a_release_with_nothing_for_this_machine_is_skipped() {
    let dir = temp_dir();
    let url = publish(&dir, "2026.11.20-1012", &tarball(&dir, "2026.11.20-1012"), None);
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("latest.json")).expect("manifest")).expect("json");
    // Built for another architecture, and with no Linux build for this one.
    manifest["archs"] = serde_json::json!(["riscv64"]);
    manifest["linux"] = serde_json::json!({});
    write_manifest(&dir, &manifest);
    assert_eq!(Channel::new(home(&dir), url).check().await.expect("check"), Checked::Skipped("2026.11.20-1012".into()));
    assert_eq!(latest(&dir), "");
}
