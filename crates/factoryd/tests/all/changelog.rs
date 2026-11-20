use crate::common::{Client, View, start, temp_dir};
use factory_protocol::v1;
use factoryd::update::{ChangelogItem, ChangelogVersion, ItemKind, parse_changelog, version_time};

fn item(kind: ItemKind, text: &str, raw: &str) -> ChangelogItem {
    ChangelogItem { kind, text: text.into(), raw: raw.into() }
}

#[test]
fn versions_come_newest_first_with_their_items_by_kind() {
    let text = "# Changelog\n\nIntro, with a - dash.\n\n## Unreleased\n- Added monitors.\n\n## 2026.11.20-1012\n- Added the Tasks panel,\n  with output.\n- Changed: agents keep their panel.\n- Fixed: no web pages.\n- Only bug fixes.\n\n## 2026.11.18-1948.30\n- First release.\n";
    assert_eq!(
        parse_changelog(text),
        [
            ChangelogVersion { version: String::new(), released: 0, items: vec![item(ItemKind::Added, "Monitors.", "Added monitors.")] },
            ChangelogVersion {
                version: "2026.11.20-1012".into(),
                released: 1_795_169_520_000,
                items: vec![
                    item(ItemKind::Added, "The Tasks panel, with output.", "Added the Tasks panel, with output."),
                    item(ItemKind::Changed, "Agents keep their panel.", "Changed: agents keep their panel."),
                    item(ItemKind::Fixed, "No web pages.", "Fixed: no web pages."),
                    item(ItemKind::Other, "Only bug fixes.", "Only bug fixes."),
                ],
            },
            ChangelogVersion {
                version: "2026.11.18-1948.30".into(),
                released: 1_795_031_310_000,
                items: vec![item(ItemKind::Other, "First release.", "First release.")],
            },
        ]
    );
}

#[test]
fn an_empty_unreleased_is_left_out() {
    let versions = parse_changelog("## Unreleased\n\n## 2026.11.20-1012\n- Fixed a crash.\n");
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].version, "2026.11.20-1012");
}

#[test]
fn a_numbered_version_says_when_it_was_released_on_its_own_line() {
    let versions = parse_changelog("## v0.1.1\nReleased 2026-11-21 09:30 UTC.\n- Fixed a crash.\n\n## v0.1.0\n- Added it.\n");
    assert_eq!(versions[0].version, "v0.1.1");
    assert_eq!(versions[0].released, 1_795_253_400_000);
    assert_eq!(versions[0].items.len(), 1);
    assert_eq!(versions[1].released, 0, "no line, no time");
}

#[test]
fn versions_name_when_they_were_released() {
    assert_eq!(version_time("2026.11.18-2123"), 1_795_036_980_000);
    assert_eq!(version_time("dev"), 0);
    assert_eq!(version_time("2026.11.18"), 0);
}

/// The repository's own changelog reads cleanly: every version has items,
/// and each item says what kind of change it is or reads as one line.
#[test]
fn the_changelog_in_the_repository_reads_cleanly() {
    let versions = parse_changelog(include_str!("../../../../docs/CHANGELOG.md"));
    assert!(versions.iter().filter(|version| !version.version.is_empty()).count() >= 3);
    for version in &versions {
        assert!(!version.items.is_empty(), "{} has no items", version.version);
        assert!(version.version.is_empty() || version.released > 0, "{} isn't a release time", version.version);
    }
    let mut released: Vec<&str> = versions.iter().map(|version| version.version.as_str()).filter(|version| !version.is_empty()).collect();
    let in_order = released.clone();
    released.sort_by(|a, b| factoryd::update::compare_versions(b, a));
    assert_eq!(released, in_order, "versions must be newest first");
}

/// The app gets the newest release's changelog with the workspace.
#[tokio::test]
async fn the_workspace_carries_the_newest_releases_changelog() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert!(workspace.changelog.is_empty());
    let mut view = View { workspace, ..View::default() };

    let release = dir.join("home/releases/2099.01.01-0000");
    std::fs::create_dir_all(&release).expect("mkdir");
    std::fs::write(release.join("factoryd"), "binary").expect("write");
    let text = "# Changelog\n\n## Unreleased\n- Added monitors.\n\n## 2026.11.20-1012\n- Fixed: no web pages.\n";
    std::fs::write(release.join("CHANGELOG.md"), text).expect("write");
    std::fs::write(dir.join("home/releases/latest"), "2099.01.01-0000").expect("write");
    view.follow(&mut client, |view| view.workspace.latest_release == "2099.01.01-0000").await;

    let item = |kind: v1::ChangelogKind, text: &str, raw: &str| v1::ChangelogItem { kind: kind.into(), text: text.into(), raw: raw.into() };
    assert_eq!(
        view.workspace.changelog,
        [
            v1::ChangelogVersion {
                version: String::new(),
                released: 0,
                items: vec![item(v1::ChangelogKind::Added, "Monitors.", "Added monitors.")],
                unreleased: true,
            },
            v1::ChangelogVersion {
                version: "2026.11.20-1012".into(),
                released: 1_795_169_520_000,
                items: vec![item(v1::ChangelogKind::Fixed, "No web pages.", "Fixed: no web pages.")],
                unreleased: false,
            },
        ]
    );
}
