use std::path::{Path, PathBuf};
use std::process::Command;

use crate::common::{Client, home, last_workspace, start, temp_dir};
use factory_protocol::v1::server_message::Body;
use factory_protocol::v1::{self, UpdateCheckState};
use factoryd::channel::{Channel, ChannelError, Checked};
use sha2::{Digest, Sha256};

fn run(command: &mut Command) {
    let status = command.status().expect("run");
    assert!(status.success(), "{command:?}");
}

/// tar, wherever this machine has it: Windows's own (bsdtar), as the
/// channel runs it, never another on PATH.
fn tar() -> Command {
    Command::new(if cfg!(target_os = "macos") {
        "/usr/bin/tar"
    } else if cfg!(windows) {
        r"C:\Windows\System32\tar.exe"
    } else {
        "tar"
    })
}

/// The backend's file name in a release: `factoryd.exe` on Windows.
fn backend() -> String {
    factoryd::home::program("factoryd")
}

/// A file's address, as curl reads it here (`file:///C:/…` on Windows).
fn file_url(path: &Path) -> String {
    if cfg!(windows) { format!("file:///{}", path.display().to_string().replace('\\', "/")) } else { format!("file://{}", path.display()) }
}

/// The file a `file_url` names.
fn url_path(url: &str) -> PathBuf {
    let path = url.trim_start_matches("file://");
    PathBuf::from(if cfg!(windows) { path.trim_start_matches('/') } else { path })
}

/// A release as `scripts/publish.sh` packs it: a signed app (on a Mac; a
/// Linux release has none), a backend and its version, in a tarball.
fn tarball(dir: &Path, version: &str) -> PathBuf {
    tarball_with(dir, version, None)
}

/// The apps host a release carries, beside factoryd.
#[derive(Clone, Copy)]
enum Host {
    Signed,
    Unsigned,
}

/// As `tarball`, with an apps host.
fn tarball_with(dir: &Path, version: &str, host: Option<Host>) -> PathBuf {
    let content = dir.join(format!("content-{version}"));
    std::fs::create_dir_all(&content).expect("content");
    let factoryd = backend();
    let apps_host = factoryd::home::program("claktory-apps");
    let mut files = vec![factoryd.as_str(), "backend-version"];
    if let Some(host) = host {
        let path = content.join(&apps_host);
        match host {
            // A real Mach-O, signed (ad hoc) as releases sign theirs.
            Host::Signed if cfg!(target_os = "macos") => {
                std::fs::copy("/usr/bin/true", &path).expect("host");
                run(Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&path));
            }
            _ => std::fs::write(&path, "not signed").expect("host"),
        }
        files.push(&apps_host);
    }
    std::fs::write(content.join(&factoryd), format!("backend {version}")).expect("factoryd");
    std::fs::write(content.join("backend-version"), format!("{version}\n")).expect("backend-version");
    let tarball = dir.join(format!("factory-{version}.tar.gz"));
    if !cfg!(target_os = "macos") {
        run(tar().arg("-czf").arg(&tarball).arg("-C").arg(&content).args(&files));
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
    files.push("Factory.app");
    run(tar().env("COPYFILE_DISABLE", "1").arg("-czf").arg(&tarball).arg("-C").arg(&content).args(&files));
    tarball
}

/// Publishes `tarball` as `version` in a channel in `dir`, for this machine
/// (as the Mac release, and as the Linux one for this architecture), and
/// returns the manifest's address.
fn publish(dir: &Path, version: &str, tarball: &Path, sha256: Option<&str>) -> String {
    let bytes = std::fs::read(tarball).expect("tarball");
    let hash: String = Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect();
    let here = serde_json::json!({
        "url": file_url(tarball),
        "sha256": sha256.unwrap_or(&hash),
        "size": bytes.len(),
    });
    let mut manifest = here.clone();
    manifest["version"] = version.into();
    manifest["min_macos"] = "12.0".into();
    manifest["archs"] = serde_json::json!(["arm64", "x86_64"]);
    manifest["linux"] = serde_json::json!({ std::env::consts::ARCH: here });
    manifest["windows"] = serde_json::json!({ std::env::consts::ARCH: here });
    write_manifest(dir, &manifest)
}

fn write_manifest(dir: &Path, manifest: &serde_json::Value) -> String {
    let path = dir.join("latest.json");
    std::fs::write(&path, manifest.to_string()).expect("manifest");
    file_url(&path)
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
    assert_eq!(std::fs::read_to_string(release.join(backend())).expect("factoryd"), "backend 2026.11.20-1012");
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
    built_here(&dir, "2026.11.21-0900");
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
async fn a_release_carries_its_apps_host_and_on_a_mac_only_a_signed_one() {
    let dir = temp_dir();
    let url = publish(&dir, "v0.1.10", &tarball_with(&dir, "v0.1.10", Some(Host::Signed)), None);
    assert_eq!(Channel::new(home(&dir), url).check().await.expect("check"), Checked::Downloaded("v0.1.10".into()));
    assert!(home(&dir).releases().join("v0.1.10").join(factoryd::home::program("claktory-apps")).is_file());

    let dir = temp_dir();
    let url = publish(&dir, "v0.1.10", &tarball_with(&dir, "v0.1.10", Some(Host::Unsigned)), None);
    let checked = Channel::new(home(&dir), url).check().await;
    if cfg!(target_os = "macos") {
        let error = checked.expect_err("an unsigned host");
        assert!(error.to_string().contains("its apps host's signature doesn't verify"), "{error}");
        assert_eq!(latest(&dir), "");
    } else {
        assert_eq!(checked.expect("Linux has no code signing"), Checked::Downloaded("v0.1.10".into()));
    }
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

/// A Windows release's backend is `factoryd.exe`: one with only a bare
/// `factoryd` (a Mac or Linux backend's name) has none.
#[cfg(windows)]
#[tokio::test]
async fn a_windows_release_without_factoryd_exe_is_refused() {
    let dir = temp_dir();
    let content = dir.join("bare");
    std::fs::create_dir_all(&content).expect("content");
    std::fs::write(content.join("factoryd"), "backend").expect("factoryd");
    let tarball = dir.join("bare.tar.gz");
    run(tar().arg("-czf").arg(&tarball).arg("-C").arg(&content).arg("factoryd"));
    let url = publish(&dir, "v0.1.10", &tarball, None);
    let error = Channel::new(home(&dir), url).check().await.expect_err("no factoryd.exe");
    assert!(error.to_string().contains("it has no factoryd.exe"), "{error}");
    assert_eq!(release_files(&dir), Vec::<String>::new());
}

/// A Windows release carries no links: one whose tarball holds a hard
/// link (a file with a second name) is refused, and nothing of it is kept.
#[cfg(windows)]
#[tokio::test]
async fn a_windows_release_holding_a_link_is_refused() {
    let dir = temp_dir();
    let content = dir.join("linked");
    std::fs::create_dir_all(&content).expect("content");
    std::fs::write(content.join("factoryd.exe"), "backend").expect("factoryd");
    std::fs::write(content.join("backend-version"), "v0.1.10\n").expect("backend-version");
    std::fs::hard_link(content.join("factoryd.exe"), content.join("claktory-apps.exe")).expect("link");
    let tarball = dir.join("linked.tar.gz");
    run(tar().arg("-czf").arg(&tarball).arg("-C").arg(&content).args(["factoryd.exe", "backend-version", "claktory-apps.exe"]));
    let url = publish(&dir, "v0.1.10", &tarball, None);
    let error = Channel::new(home(&dir), url).check().await.expect_err("a link");
    assert!(error.to_string().contains("it holds a link"), "{error}");
    assert_eq!(release_files(&dir), Vec::<String>::new());
}

/// A Factory on a date-stamped release (as every one was before v0.1.0) gets
/// the first numbered one, and numbered releases keep their order past 9.
#[tokio::test]
async fn numbered_releases_are_newer_than_date_stamps_and_ordered_by_number() {
    let dir = temp_dir();
    let releases = home(&dir).releases();
    for old in ["2026.11.19-1958", "v0.1.8", "v0.1.9"] {
        std::fs::create_dir_all(releases.join(old)).expect("release");
        std::fs::write(releases.join(old).join(backend()), "old").expect("factoryd");
    }
    std::fs::write(releases.join("latest"), "v0.1.9\n").expect("latest");
    let url = publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    assert_eq!(Channel::new(home(&dir), url).check().await.expect("check"), Checked::Downloaded("v0.1.10".into()));
    // The oldest goes: the date stamp, not v0.1.10 ("v0.1.10" < "v0.1.9" as text).
    assert_eq!(release_files(&dir), ["latest", "v0.1.10", "v0.1.8", "v0.1.9"]);

    let dir = temp_dir();
    let releases = home(&dir).releases();
    std::fs::create_dir_all(releases.join("2026.11.19-1958")).expect("release");
    std::fs::write(releases.join("2026.11.19-1958").join(backend()), "old").expect("factoryd");
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
        std::fs::write(releases.join(old).join(backend()), "old").expect("factoryd");
    }
    std::fs::write(releases.join("latest"), "2026.11.19-0900\n").expect("latest");
    let url = publish(&dir, "2026.11.20-1012", &tarball(&dir, "2026.11.20-1012"), None);
    assert_eq!(Channel::new(home(&dir), url).check().await.expect("check"), Checked::Downloaded("2026.11.20-1012".into()));
    assert_eq!(release_files(&dir), ["2026.11.18-0900", "2026.11.19-0900", "2026.11.20-1012", "latest"]);
}

/// A release built here, as `scripts/release.sh` leaves it (with the
/// commit it was built from).
fn built_here(dir: &Path, version: &str) {
    let release = home(dir).releases().join(version);
    std::fs::create_dir_all(&release).expect("release");
    std::fs::write(release.join(backend()), format!("backend {version}")).expect("factoryd");
    std::fs::write(release.join("source"), "0123abcd\n").expect("source");
}

/// Stamps the channel's manifest in `dir` as `scripts/publish.sh` does
/// (`issued_ms`), and returns its bytes.
fn stamp(dir: &Path, issued_ms: u64) -> Vec<u8> {
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("latest.json")).expect("manifest")).expect("json");
    manifest["issued_ms"] = issued_ms.into();
    write_manifest(dir, &manifest);
    std::fs::read(dir.join("latest.json")).expect("manifest")
}

/// Publishes `version` in the channel in `dir`, stamped `issued_ms`.
fn publish_stamped(dir: &Path, version: &str, issued_ms: u64) -> Vec<u8> {
    publish(dir, version, &tarball(dir, version), None);
    stamp(dir, issued_ms)
}

/// Serves `bytes` as the channel's manifest again (a replay).
fn serve(dir: &Path, bytes: &[u8]) {
    std::fs::write(dir.join("latest.json"), bytes).expect("manifest");
}

fn stale(checked: Result<Checked, ChannelError>) -> bool {
    matches!(checked, Err(ChannelError::Stale(_)))
}

/// #456: a release pulled from the channel after it was downloaded (a
/// newer manifest moved back) is deleted and no longer offered; releases
/// built here stay.
#[tokio::test]
async fn a_release_the_channel_pulled_is_deleted_and_no_longer_offered() {
    let dir = temp_dir();
    let releases = home(&dir).releases();
    built_here(&dir, "v0.1.9");
    std::fs::write(releases.join("latest"), "v0.1.9\n").expect("latest");
    let url = publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    stamp(&dir, 1000);
    let mut channel = Channel::new(home(&dir), url.clone());
    assert_eq!(channel.check().await.expect("check"), Checked::Downloaded("v0.1.10".into()));
    assert_eq!(std::fs::read_to_string(releases.join("v0.1.10/downloaded-from")).expect("marked"), format!("{url}\n1000\n"));
    // While the channel offers it, it stays.
    assert_eq!(channel.check().await.expect("check"), Checked::UpToDate);
    assert_eq!(release_files(&dir), ["latest", "v0.1.10", "v0.1.9"]);

    // Then: a newer one built here, and one an older backend downloaded
    // (no mark, but not built here either).
    built_here(&dir, "v0.1.11");
    std::fs::create_dir_all(releases.join("v0.1.12")).expect("release");
    std::fs::write(releases.join("v0.1.12").join(backend()), "backend v0.1.12").expect("factoryd");
    std::fs::write(releases.join("latest"), "v0.1.12\n").expect("latest");

    // The channel moves back to v0.1.9, in a newer manifest: what it gave
    // and no longer offers goes, and `latest` names the newest release left.
    publish_stamped(&dir, "v0.1.9", 2000);
    assert_eq!(channel.check().await.expect("check"), Checked::UpToDate);
    assert_eq!(release_files(&dir), ["latest", "v0.1.11", "v0.1.9"]);
    assert_eq!(latest(&dir), "v0.1.11");
}

/// #478: an old signed manifest served again is refused, so it can neither
/// roll the offer back nor take back a newer release; a newer one can.
#[tokio::test]
async fn a_replayed_manifest_is_refused_and_withdraws_nothing() {
    let dir = temp_dir();
    let releases = home(&dir).releases();
    built_here(&dir, "v0.1.9");
    std::fs::write(releases.join("latest"), "v0.1.9\n").expect("latest");
    let url = publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    let first = stamp(&dir, 1000);
    let mut channel = Channel::new(home(&dir), url.clone());
    assert_eq!(channel.check().await.expect("check"), Checked::Downloaded("v0.1.10".into()));
    let between = publish_stamped(&dir, "v0.1.9", 2000);
    let newest = publish_stamped(&dir, "v0.1.11", 3000);
    // 2000 was published between, and 3000 replaced it before a check.
    assert_eq!(channel.check().await.expect("check"), Checked::Downloaded("v0.1.11".into()));
    let everything = ["latest", "v0.1.10", "v0.1.11", "v0.1.9"];
    assert_eq!(release_files(&dir), everything);

    // Replays: the first manifest, and one stamped between that moved back.
    serve(&dir, &first);
    assert!(stale(channel.check().await));
    serve(&dir, &between);
    assert!(stale(channel.check().await));
    // Unstamped, once the channel's are.
    publish(&dir, "v0.1.9", &tarball(&dir, "v0.1.9"), None);
    assert!(stale(channel.check().await));
    // Another manifest with the newest one's stamp.
    publish_stamped(&dir, "v0.1.9", 3000);
    assert!(stale(channel.check().await));
    assert_eq!(release_files(&dir), everything);
    assert_eq!(latest(&dir), "v0.1.11");

    // The newest one read again is fine.
    serve(&dir, &newest);
    assert_eq!(channel.check().await.expect("the same manifest"), Checked::UpToDate);

    // After a restart, still refused.
    let mut channel = Channel::new(home(&dir), url.clone());
    serve(&dir, &between);
    assert!(stale(channel.check().await));
    assert_eq!(release_files(&dir), everything);

    // A newer manifest that moves back withdraws both: their manifests are
    // older than it.
    publish_stamped(&dir, "v0.1.9", 4000);
    assert_eq!(channel.check().await.expect("check"), Checked::UpToDate);
    assert_eq!(release_files(&dir), ["latest", "v0.1.9"]);
    assert_eq!(latest(&dir), "v0.1.9");

    // Another channel starts afresh.
    let other = temp_dir();
    let other_url = publish(&other, "v0.1.9", &tarball(&other, "v0.1.9"), None);
    assert_eq!(Channel::new(home(&dir), other_url).check().await.expect("another channel"), Checked::UpToDate);
}

/// `scripts/publish.sh --repoint`: the older release's own manifest, only
/// stamped anew, takes the newer release back; the newer one's manifest
/// served again afterwards is refused.
#[tokio::test]
async fn a_repointed_manifest_takes_the_newer_release_back() {
    let dir = temp_dir();
    let url = publish(&dir, "v0.1.20", &tarball(&dir, "v0.1.20"), None);
    let v1 = stamp(&dir, 1000);
    let mut channel = Channel::new(home(&dir), url);
    assert_eq!(channel.check().await.expect("check"), Checked::Downloaded("v0.1.20".into()));
    let v2 = publish_stamped(&dir, "v0.1.21", 2000);
    assert_eq!(channel.check().await.expect("check"), Checked::Downloaded("v0.1.21".into()));

    let mut repointed: serde_json::Value = serde_json::from_slice(&v1).expect("json");
    repointed["issued_ms"] = 3000.into();
    write_manifest(&dir, &repointed);
    assert_eq!(channel.check().await.expect("repointed"), Checked::UpToDate, "nothing new offered");
    assert_eq!(release_files(&dir), ["latest", "v0.1.20"]);
    assert_eq!(latest(&dir), "v0.1.20");

    serve(&dir, &v2);
    assert!(stale(channel.check().await));
    serve(&dir, &v1);
    assert!(stale(channel.check().await));
    assert_eq!(release_files(&dir), ["latest", "v0.1.20"]);
}

/// Before stamps: an unstamped manifest is taken as before, but never takes
/// a release back.
#[tokio::test]
async fn an_unstamped_manifest_is_offered_but_withdraws_nothing() {
    let dir = temp_dir();
    let url = publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    let mut channel = Channel::new(home(&dir), url);
    assert_eq!(channel.check().await.expect("check"), Checked::Downloaded("v0.1.10".into()));
    assert_eq!(release_files(&dir), ["latest", "v0.1.10"]);
    publish(&dir, "v0.1.9", &tarball(&dir, "v0.1.9"), None);
    assert_eq!(channel.check().await.expect("check"), Checked::UpToDate);
    assert_eq!(release_files(&dir), ["latest", "v0.1.10"]);
    assert_eq!(latest(&dir), "v0.1.10");
}

/// The newest stamp is kept in a file of its own; one that can't be read
/// counts as none seen, never as a reason to refuse every manifest.
#[tokio::test]
async fn a_damaged_stamp_file_starts_again() {
    let dir = temp_dir();
    let url = publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    stamp(&dir, 1000);
    let mut channel = Channel::new(home(&dir), url.clone());
    assert_eq!(channel.check().await.expect("check"), Checked::Downloaded("v0.1.10".into()));
    let file = home(&dir).root().join("update-channel.json");
    let kept: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).expect("kept")).expect("json");
    assert_eq!(kept["channels"][&url]["issued_ms"], 1000);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).expect("file").permissions().mode() & 0o777, 0o600);
    }

    std::fs::write(&file, "not json").expect("damage");
    publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    assert_eq!(channel.check().await.expect("unstamped, as before stamps"), Checked::UpToDate);
    stamp(&dir, 500);
    assert_eq!(channel.check().await.expect("stamped"), Checked::UpToDate);
    let kept: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).expect("kept")).expect("json");
    assert_eq!(kept["channels"][&url]["issued_ms"], 500);
}

#[tokio::test]
async fn a_pulled_release_stops_being_offered() {
    let channel = temp_dir();
    let url = publish(&channel, "v0.1.10", &tarball(&channel, "v0.1.10"), None);
    stamp(&channel, 1000);
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.use_channel(&url);
    let (mut client, _, _) = Client::connect(addr).await;
    factory.check_on_schedule().await;
    let offered = client
        .wait_for(|body| match body {
            Body::Workspace(workspace) if !workspace.latest_release.is_empty() => Some(workspace.latest_release.clone()),
            _ => None,
        })
        .await;
    assert_eq!(offered, "v0.1.10");

    // Pulled: a newer manifest names an older release, which can't run here.
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(channel.join("latest.json")).expect("manifest")).expect("json");
    manifest["version"] = "v0.1.9".into();
    manifest["archs"] = serde_json::json!(["riscv64"]);
    manifest["linux"] = serde_json::json!({});
    manifest["windows"] = serde_json::json!({});
    manifest["issued_ms"] = 2000.into();
    write_manifest(&channel, &manifest);
    factory.check_on_schedule().await;
    client
        .wait_for(|body| match body {
            Body::Workspace(workspace) if workspace.latest_release.is_empty() => Some(()),
            _ => None,
        })
        .await;
    assert_eq!(release_files(&dir), Vec::<String>::new());
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
    manifest["windows"] = serde_json::json!({});
    write_manifest(&dir, &manifest);
    assert_eq!(Channel::new(home(&dir), url).check().await.expect("check"), Checked::Skipped("2026.11.20-1012".into()));
    assert_eq!(latest(&dir), "");
}

/// Signs the channel's manifest in `dir` as `scripts/publish.sh` does:
/// `latest.json.sig` beside it, `<key id>:<base64 ed25519 signature>`.
fn sign(dir: &Path, id: &str, key: &ed25519_dalek::SigningKey) {
    use base64::Engine;
    use ed25519_dalek::Signer;
    let manifest = std::fs::read(dir.join("latest.json")).expect("manifest");
    let signature = base64::engine::general_purpose::STANDARD.encode(key.sign(&manifest).to_bytes());
    std::fs::write(dir.join("latest.json.sig"), format!("{id}:{signature}\n")).expect("signature");
}

fn release_key(seed: u8) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[seed; 32])
}

fn pinned() -> Vec<(String, ed25519_dalek::VerifyingKey)> {
    vec![("claktory-release-1".into(), release_key(1).verifying_key()), ("claktory-recovery-1".into(), release_key(2).verifying_key())]
}

#[tokio::test]
async fn a_signed_channel_takes_a_release_only_with_its_keys_signature() {
    let dir = temp_dir();
    let url = publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    let mut channel = Channel::new(home(&dir), url).signed_by(pinned());

    // No signature beside it: nothing is downloaded, and it's not given up on.
    let error = channel.check().await.expect_err("unsigned");
    assert!(matches!(error, ChannelError::Unsigned(_)), "{error}");
    assert_eq!(release_files(&dir), Vec::<String>::new());

    // Signed by a key nobody pinned.
    sign(&dir, "claktory-release-1", &release_key(9));
    assert!(matches!(channel.check().await, Err(ChannelError::Unsigned(_))));
    assert_eq!(latest(&dir), "");

    // Signed, then the manifest changed (another tarball's hash, say).
    sign(&dir, "claktory-release-1", &release_key(1));
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("latest.json")).expect("manifest")).expect("json");
    manifest["sha256"] = "0".repeat(64).into();
    write_manifest(&dir, &manifest);
    assert!(matches!(channel.check().await, Err(ChannelError::Unsigned(_))));
    assert_eq!(release_files(&dir), Vec::<String>::new());

    // Published again and signed by the release key: taken.
    publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    sign(&dir, "claktory-release-1", &release_key(1));
    assert_eq!(channel.check().await.expect("signed"), Checked::Downloaded("v0.1.10".into()));
    assert_eq!(latest(&dir), "v0.1.10");
}

#[tokio::test]
async fn the_recovery_key_signs_a_release_too() {
    let dir = temp_dir();
    let url = publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    sign(&dir, "claktory-recovery-1", &release_key(2));
    let checked = Channel::new(home(&dir), url).signed_by(pinned()).check().await;
    assert_eq!(checked.expect("signed by the recovery key"), Checked::Downloaded("v0.1.10".into()));
}

/// A channel in a folder (how dev and local channels are tried) needs no
/// signature; an online one always does.
#[tokio::test]
async fn only_a_channel_in_a_folder_is_taken_unsigned() {
    let dir = temp_dir();
    let url = publish(&dir, "v0.1.10", &tarball(&dir, "v0.1.10"), None);
    assert!(!factoryd::channel::requires_signature(&url));
    assert_eq!(Channel::new(home(&dir), url).check().await.expect("a folder's"), Checked::Downloaded("v0.1.10".into()));
    assert!(factoryd::channel::requires_signature("https://github.com/claktor/claktory/releases/latest/download/latest.json"));
}

/// What the Workspaces say of Check for updates, until `done` likes one.
async fn check_until(client: &mut Client, done: impl Fn(&v1::UpdateCheck) -> bool) -> (Vec<UpdateCheckState>, v1::UpdateCheck) {
    let mut seen = Vec::new();
    let last = client
        .wait_for(|body| match body {
            Body::Workspace(workspace) => {
                let check = workspace.update_check.clone().unwrap_or_default();
                if seen.last() != Some(&check.state()) {
                    seen.push(check.state());
                }
                done(&check).then_some(check)
            }
            _ => None,
        })
        .await;
    (seen, last)
}

#[tokio::test]
async fn check_for_updates_downloads_a_newer_release_and_says_so() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.update_check.unwrap_or_default().state(), UpdateCheckState::None, "nobody asked yet");

    // No channel (a build that wasn't published) and no release here.
    let asked = client.ok(check()).await;
    let checking = last_workspace(&asked).and_then(|workspace| workspace.update_check.clone()).unwrap_or_default();
    assert_eq!(checking.state(), UpdateCheckState::Checking, "shown at once");
    let (_, done) = check_until(&mut client, |check| check.state() == UpdateCheckState::UpToDate).await;
    assert_eq!(done.version, "dev");
    assert!(done.checked_ms > 0);

    // Asked again at once: its answer stands, nothing is fetched.
    let again = client.ok(check()).await;
    assert!(last_workspace(&again).is_none_or(|workspace| workspace.update_check.as_ref() == Some(&done)), "{again:?}");

    // A channel with a newer release: it downloads, then offers it.
    drop(factory);
    let other = temp_dir();
    let url = publish(&other, "v0.1.10", &tarball(&other, "v0.1.10"), None);
    let fresh = temp_dir();
    let (addr, factory) = start(&fresh).await;
    factory.use_channel(&url);
    let (mut client, _, _) = Client::connect(addr).await;
    client.ok(check()).await;
    let (seen, done) =
        check_until(&mut client, |check| matches!(check.state(), UpdateCheckState::Ready | UpdateCheckState::Failed)).await;
    assert_eq!(done.state(), UpdateCheckState::Ready, "{done:?}");
    assert_eq!(done.version, "v0.1.10");
    assert!(seen.contains(&UpdateCheckState::Downloading), "{seen:?}");
    assert_eq!(latest(&fresh), "v0.1.10");
}

#[tokio::test]
async fn check_for_updates_says_why_it_couldnt() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.use_channel(&file_url(&dir.join("nothing-here.json")));
    let (mut client, _, _) = Client::connect(addr).await;
    client.ok(check()).await;
    let (_, done) = check_until(&mut client, |check| matches!(check.state(), UpdateCheckState::Failed | UpdateCheckState::UpToDate)).await;
    assert_eq!(done.state(), UpdateCheckState::Failed, "{done:?}");
    assert_eq!(done.error, "Claktory couldn't reach the update server. Check your internet connection, then try again.");

    // The schedule's next check fails too: the answer stays, never "up to
    // date". Then the channel is back with a newer release: it's offered.
    factory.check_on_schedule().await;
    let channel = temp_dir();
    let url = publish(&channel, "v0.1.10", &tarball(&channel, "v0.1.10"), None);
    std::fs::rename(url_path(&url), dir.join("nothing-here.json")).expect("move the manifest");
    factory.check_on_schedule().await;
    let (seen, done) = check_until(&mut client, |check| check.state() == UpdateCheckState::Ready).await;
    assert!(!seen.contains(&UpdateCheckState::UpToDate), "{seen:?}");
    assert_eq!(done.version, "v0.1.10");
}

fn check() -> v1::client_message::Body {
    v1::client_message::Body::CheckForUpdates(v1::CheckForUpdates {})
}
