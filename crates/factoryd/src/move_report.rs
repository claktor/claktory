//! After a move: what Rivet checks on the new computer and tells
//! the person, with `move_report` (the Move extension), while the copy left
//! behind can still be gone back to. One report in plain words: Claktory
//! runs here; the vault (the person's secrets) is unlocked, or waits to be;
//! the account checked in after the move gave it this Claktory's identity;
//! which devices connected since;
//! each project's folders where the move put them; each signed-in account
//! still works (a usage read, so nothing is spent, and a refused one is
//! marked Sign in again); tools the agents used that aren't here; and what
//! of each project only builds on a Mac. The platform lines come first on
//! the old computer too, in move_claktory's answer, before anything moves.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use factory_protocol::v1;
use rusqlite::OptionalExtension;

use crate::config::Config;
use crate::factory::Factory;
use crate::usage::SignInOutcome;

/// The move into this Claktory, from factory.db's `moves`.
struct Moved {
    machine: String,
    version: String,
    /// When it was imported here, in ms since 1970.
    at: i64,
    /// The paths it was moved by: [[from, to], …].
    map: Vec<(String, String)>,
}

fn last_move(factory: &Factory) -> Option<Moved> {
    let db = factory.store.db();
    db.query_row("SELECT machine, version, imported, map FROM moves ORDER BY imported DESC LIMIT 1", [], |row| {
        let map: String = row.get(3)?;
        Ok(Moved { machine: row.get(0)?, version: row.get(1)?, at: row.get(2)?, map: serde_json::from_str(&map).unwrap_or_default() })
    })
    .optional()
    .ok()
    .flatten()
}

/// "12:04 on 22 Nov", here.
fn when(ms: i64) -> String {
    crate::transcript::local_time(ms, "%H:%M on %-d %b")
}

/// The report on this Claktory after a move, for Rivet to tell the person.
pub(crate) async fn report(factory: &Factory) -> String {
    let here =
        format!("{} ({})", crate::remote::machine_name(), crate::move_import::platform(std::env::consts::OS, std::env::consts::ARCH));
    let mut text = String::new();
    let Some(moved) = last_move(factory) else {
        return format!(
            "This Claktory hasn't moved here: it has no record of a move. It runs on {here}, Claktory {}.",
            crate::update::VERSION
        );
    };
    let _ = writeln!(
        text,
        "Claktory moved here from {} ({}) at {}. It now runs on {here}, Claktory {}.",
        moved.machine,
        moved.version,
        when(moved.at),
        crate::update::VERSION
    );
    let free = crate::uploads::available(factory.home.root()).unwrap_or(0);
    if free > 0 {
        let _ = writeln!(text, "Free disk: {} GB.", free / 1_000_000_000);
    }

    // The vault first: sign-ins kept in it can't be checked while it's locked.
    let (config, account, devices, locked_why) = {
        let inner = factory.lock();
        let problem = factory.account.view(&inner.config.account).map(|account| account.problem).unwrap_or_default();
        (
            inner.config.clone(),
            factory.account.view(&inner.config.account),
            factory.remote_view(&inner).devices,
            crate::vault::signins::locked_why(&factory.vault, problem),
        )
    };
    let vault_open = factory.vault.is_open();
    if !factory.vault.exists() {
        text.push_str("Secrets: none to unlock.\n");
    } else if vault_open {
        text.push_str("Secrets: unlocked.\n");
    } else {
        let _ = writeln!(
            text,
            "Secrets: locked. {locked_why} Agents that need a sign-in kept with them wait, and carry on by themselves once they're unlocked."
        );
    }

    let _ = writeln!(text, "{}", account_line(account.as_ref(), moved.at));
    let _ = writeln!(text, "{}", devices_line(&devices, moved.at));

    for line in folder_lines(&config, &moved.map, &std::env::home_dir().unwrap_or_default()) {
        let _ = writeln!(text, "{line}");
    }

    // Each signed-in account, read once: only once the vault is open, since
    // a read while it's locked says nothing.
    if factory.vault.exists() && !vault_open {
        text.push_str("Sign-ins: checked once your secrets are unlocked; ask for this report again then.\n");
    } else {
        let checks = factory.check_sign_ins().await;
        if checks.is_empty() {
            text.push_str("Sign-ins: none to check.\n");
        }
        for check in checks {
            let who = if check.account.is_empty() { check.provider.clone() } else { format!("{} ({})", check.provider, check.account) };
            let how = match check.outcome {
                SignInOutcome::Works => "works here".to_owned(),
                SignInOutcome::NoLimits => "signed in with a key, which has no usage to read; nothing says it doesn't work".to_owned(),
                SignInOutcome::Refused => "was refused here: sign in again in Settings → Providers (Needs you shows it too)".to_owned(),
                SignInOutcome::Locked => "waits until your secrets are unlocked".to_owned(),
                SignInOutcome::Failed(why) => format!("couldn't be checked: {why}"),
            };
            let _ = writeln!(text, "Sign-in: {who} {how}.");
        }
    }

    let missing = missing_tools(factory, &config);
    if !missing.is_empty() {
        let tools: Vec<String> = missing
            .iter()
            .map(|(tool, agents)| {
                let names: Vec<&str> = agents.iter().map(String::as_str).collect();
                format!("{tool} ({} {})", join_and(&names), if names.len() == 1 { "uses it" } else { "use it" })
            })
            .collect();
        let _ = writeln!(text, "Not installed here: {}.", tools.join("; "));
    }

    text.push('\n');
    text.push_str(&platform_lines(&projects_of(&config), std::env::consts::OS, true));
    text.trim_end().to_owned()
}

fn account_line(account: Option<&v1::ClaktoryAccount>, moved_at: i64) -> String {
    use v1::ClaktoryAccountState as State;
    let Some(account) = account else {
        return "Account: signing in to claktory.com is off here.".to_owned();
    };
    let service = if account.service.is_empty() { "claktory.com".to_owned() } else { service_host(&account.service) };
    match account.state() {
        State::SignedIn if account.last_check_in_ms >= moved_at => format!(
            "Account: checked in with {service} at {}, so your account knows this Claktory is here now.",
            when(account.last_check_in_ms)
        ),
        State::SignedIn if account.problem.is_empty() => {
            format!("Account: hasn't checked in with {service} since the move yet; it will by itself within minutes.")
        }
        State::SignedIn => format!("Account: hasn't checked in with {service} since the move: {}", account.problem),
        State::Revoked => {
            format!("Account: {service} says this Claktory was removed from your account. Sign in again in Settings → Account.")
        }
        _ => format!("Account: this Claktory isn't signed in to {service}. Sign in from Settings → Account."),
    }
}

fn service_host(service: &str) -> String {
    if service == crate::account::DEFAULT_SERVICE {
        return "claktory.com".to_owned();
    }
    reqwest::Url::parse(service).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_else(|| service.to_owned())
}

fn devices_line(devices: &[v1::PairedDevice], moved_at: i64) -> String {
    if devices.is_empty() {
        return "Devices: none added.".to_owned();
    }
    let (seen, not_yet): (Vec<&v1::PairedDevice>, Vec<&v1::PairedDevice>) =
        devices.iter().partition(|device| device.connected || device.last_seen_ms >= moved_at);
    let named = |list: &[&v1::PairedDevice]| join_and(&list.iter().map(|device| device.name.as_str()).collect::<Vec<_>>());
    let mut line = if seen.is_empty() {
        "Devices: none connected here since the move yet.".to_owned()
    } else {
        format!("Devices connected here since the move: {}.", named(&seen))
    };
    if !not_yet.is_empty() {
        let how = if not_yet.len() == 1 {
            "which connects by itself the next time it's opened"
        } else {
            "which each connect by themselves the next time they're opened"
        };
        let _ = write!(line, " Not yet: {}, {how}.", named(&not_yet));
    }
    line
}

/// Each project's folders, where the move put them.
/// `map` is the move's, longest prefix first; `user_home` is this user's
/// home folder, where the old one's went.
fn folder_lines(config: &Config, map: &[(String, String)], user_home: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let mut missing = Vec::new();
    let mut count = 0;
    for (name, paths) in projects_of(config) {
        for path in paths {
            count += 1;
            if !path.is_dir() {
                missing.push(format!("{name} ({})", path.display()));
            }
        }
    }
    if count == 0 {
        lines.push("Folders: no projects.".to_owned());
    } else if missing.is_empty() {
        lines.push(format!("Folders: all {count} of the projects' folders are in place."));
    } else {
        let are = if missing.len() == 1 { "isn't" } else { "aren't" };
        lines.push(format!("Folders: {} of {count} {are} here: {}.", missing.len(), missing.join("; ")));
    }
    // The home folder first: the pair that ends in this user's home; then
    // any other path moved (folders from outside the old home folder, say),
    // unless it only followed the home folder (Claktory's own folder in it).
    let home = map.iter().find(|(_, to)| Path::new(to) == user_home);
    if let Some((from, to)) = home.filter(|(from, to)| from != to) {
        lines.push(format!("Home folder: {from} is now {to}."));
    }
    let others: Vec<String> = map
        .iter()
        .filter(|pair| Some(*pair) != home)
        .filter(|(from, to)| {
            from != to
                && !home.is_some_and(|(home_from, home_to)| {
                    Path::new(from).strip_prefix(home_from).is_ok_and(|rest| Path::new(home_to).join(rest) == Path::new(to))
                })
        })
        .map(|(from, to)| format!("{from} is now {to}"))
        .collect();
    if !others.is_empty() {
        lines.push(format!("Also moved: {}.", others.join("; ")));
    }
    lines
}

/// Each project, by name, with its own folder and its folders' (each once).
pub(crate) fn projects_of(config: &Config) -> Vec<(String, Vec<PathBuf>)> {
    config
        .projects
        .iter()
        .map(|project| {
            let mut paths = vec![project.path.clone()];
            for folder in &project.folders {
                if !paths.contains(&folder.path) {
                    paths.push(folder.path.clone());
                }
            }
            (project.name.clone(), paths)
        })
        .collect()
}

/// Tools the agents' background tasks used that this computer hasn't got,
/// each with the agents that used it.
fn missing_tools(factory: &Factory, config: &Config) -> BTreeMap<String, Vec<String>> {
    let db = factory.store.db();
    let mut missing: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for agent in &config.agents {
        for tool in crate::move_import::missing_tools(&db, &agent.id) {
            missing.entry(tool).or_default().push(agent.name.clone());
        }
    }
    missing
}

/// What of each project needs a Mac, when `os` (the computer it's on, or
/// moves to) isn't one; led by "For most projects this just works".
/// `moved`: said after the move, else before it.
pub(crate) fn platform_lines(projects: &[(String, Vec<PathBuf>)], os: &str, moved: bool) -> String {
    let mut text = String::from("For most projects this just works: their files, history and settings are the same on the new computer.");
    let exceptions: Vec<String> = if os == "macos" {
        Vec::new()
    } else {
        projects
            .iter()
            .filter_map(|(name, paths)| {
                let found: BTreeSet<&str> = paths.iter().flat_map(|path| mac_only(path)).collect();
                (!found.is_empty()).then(|| {
                    let found: Vec<&str> = MAC_ONLY.iter().copied().filter(|label| found.contains(label)).collect();
                    format!(
                        "{name} has {}, which build only on a Mac (Xcode, the Simulator, signing). The rest of it works {}; those parts still need a Mac.",
                        join_and(&found),
                        if moved { "here" } else { "there" }
                    )
                })
            })
            .collect()
    };
    if exceptions.is_empty() {
        text.push_str(" That's true of all of yours.");
    }
    for line in exceptions {
        let _ = write!(text, "\n{line}");
    }
    let _ = write!(
        text,
        "\nBuild output and caches (such as target/ and node_modules/) {}, since they'd be useless on another computer, so each project's first build {} takes longer.",
        if moved { "weren't copied" } else { "aren't copied" },
        if moved { "here" } else { "there" }
    );
    text
}

fn join_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// What a project can have that needs a Mac, in the order it's said.
const MAC_ONLY: [&str; 4] = ["an iOS app", "a Mac app", "an Xcode project", "iOS build scripts"];

/// Folders and files a project keeps that need a Mac: what each is, at
/// most two levels down, without caches and history.
fn mac_only(dir: &Path) -> BTreeSet<&'static str> {
    const SKIP: [&str; 8] = [".git", "node_modules", "target", "build", "Pods", ".dart_tool", "DerivedData", ".build"];
    let mut found = BTreeSet::new();
    let mut look = vec![(dir.to_path_buf(), 0)];
    while let Some((at, depth)) = look.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else { continue };
        for entry in entries.flatten().take(500) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            if is_dir && SKIP.contains(&name.as_str()) {
                continue;
            }
            if name.ends_with(".xcodeproj") || name.ends_with(".xcworkspace") {
                found.insert("an Xcode project");
                continue;
            }
            match (name.as_str(), is_dir) {
                ("ios", true) => {
                    found.insert("an iOS app");
                }
                ("macos", true) => {
                    found.insert("a Mac app");
                }
                ("ios.sh", false) => {
                    found.insert("iOS build scripts");
                }
                _ => {}
            }
            if is_dir && depth < 1 {
                look.push((entry.path(), depth + 1));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_from_outside_the_home_folder_are_told_where_they_went() {
        let map = [
            ("/Volumes/Data/Assets", "/home/you/moved-from-mac/Volumes/Data/Assets"),
            ("/Users/me/.factory", "/home/you/.factory"),
            ("/Users/me", "/home/you"),
        ]
        .map(|(from, to)| (from.to_owned(), to.to_owned()));
        let lines = folder_lines(&Config::default(), &map, Path::new("/home/you"));
        assert_eq!(
            lines,
            [
                "Folders: no projects.",
                "Home folder: /Users/me is now /home/you.",
                "Also moved: /Volumes/Data/Assets is now /home/you/moved-from-mac/Volumes/Data/Assets."
            ]
        );
    }

    #[test]
    fn mac_only_parts_are_named_for_a_computer_that_isnt_a_mac() {
        let dir = std::env::temp_dir().join(format!("move-report-{}", std::process::id()));
        let app = dir.join("app");
        std::fs::create_dir_all(app.join("ios/Runner.xcodeproj")).expect("ios");
        std::fs::create_dir_all(app.join("macos")).expect("macos");
        std::fs::create_dir_all(dir.join("scripts")).expect("scripts");
        std::fs::write(dir.join("scripts/ios.sh"), "").expect("ios.sh");
        std::fs::create_dir_all(dir.join("node_modules/x/ios")).expect("a cache");
        let site = std::env::temp_dir().join(format!("move-report-site-{}", std::process::id()));
        std::fs::create_dir_all(site.join("src")).expect("site");
        let projects = vec![("Claktory".to_owned(), vec![dir.clone()]), ("Site".to_owned(), vec![site.clone()])];

        let linux = platform_lines(&projects, "linux", true);
        assert!(linux.starts_with("For most projects this just works"), "{linux}");
        assert!(linux.contains("\nClaktory has an iOS app, a Mac app and iOS build scripts, which build only on a Mac"), "{linux}");
        assert!(!linux.contains("Site has"), "{linux}");
        assert!(linux.contains("weren't copied"), "{linux}");
        let mac = platform_lines(&projects, "macos", false);
        assert!(mac.contains("That's true of all of yours.") && mac.contains("aren't copied"), "{mac}");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&site);
    }
}
