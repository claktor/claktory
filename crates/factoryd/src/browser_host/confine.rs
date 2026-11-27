//! Agents' commands on a Mac kept from what runs Claktory's browser: once
//! the browser is set up on this Mac,
//! every agent command (the shell's, background tasks', monitors') runs
//! under a Seatbelt profile that allows everything but Apple's container
//! tool (its services, `com.apple.container.*`) and its folder, where the
//! browser's VM and the volume that keeps its sessions are; the file tools
//! are denied that folder too (`denied`). factoryd itself, which runs the
//! VM, isn't under it.
//!
//! Defence in depth, not a wall: an agent's command can still have
//! something run outside it (launchd, another app, a shell rc file), which
//! only D's containment closes. Nothing claims otherwise.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The program that applies a Seatbelt profile to what it runs.
const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// Apple's container tool's folder for this person: its VMs, images and
/// volumes (the browser's among them), and its sockets.
pub(crate) fn data_root() -> Option<PathBuf> {
    let root = super::account_home()?.join("Library/Application Support/com.apple.container");
    Some(std::fs::canonicalize(&root).unwrap_or(root))
}

/// Whether Claktory's browser is set up on this Mac, for any Claktory of
/// this person's: a volume of its is there (`claktory-browser…`).
fn set_up(root: &Path) -> bool {
    std::fs::read_dir(root.join("volumes"))
        .map(|entries| entries.flatten().any(|entry| entry.file_name().to_string_lossy().starts_with("claktory-browser")))
        .unwrap_or(false)
}

/// The profile: everything allowed but the container tool's services and
/// `root`. The path is quoted, so a folder named `") (allow default` can't
/// break out.
pub(crate) fn profile(root: &Path) -> String {
    let root = root.to_string_lossy();
    format!(
        "(version 1)
(allow default)
; Claktory's browser: Apple's container tool and its folder are Claktory's alone.
(deny mach-lookup (global-name-prefix \"com.apple.container.\"))
(deny file-read* file-write* (subpath {quoted}))
(deny network-outbound (remote unix-socket (path-regex {regex})))
",
        quoted = string(&root),
        regex = string(&format!("^{}/", literal_regex(&root))),
    )
}

/// `text` in quotes, with `\` and `"` escaped.
fn string(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        if matches!(c, '\\' | '"') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// `text` escaped for a regular expression, so every character matches
/// only itself.
fn literal_regex(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\^$.|?*+()[]{}".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// What an agent command starting now runs under: the profile, once the
/// browser is set up on this Mac; nothing before, or without sandbox-exec
/// (said once).
fn prefix(root: &Path) -> Option<Vec<OsString>> {
    if !set_up(root) {
        return None;
    }
    if !Path::new(SANDBOX_EXEC).is_file() {
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| tracing::warn!("this Mac has no sandbox-exec, so agents' commands aren't kept from Claktory's browser's VM"));
        return None;
    }
    // Seatbelt matches real paths: canonical now that the folder is there,
    // even when it wasn't yet at start.
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    Some(vec![SANDBOX_EXEC.into(), "-p".into(), profile(&root).into()])
}

/// From now on, agents' commands run under `prefix` (at start, where the
/// browser is on, on a Mac).
pub(crate) fn install() {
    let Some(root) = data_root() else { return };
    factory_agent::tools::confine::set(Some(std::sync::Arc::new(move || prefix(&root))));
}

/// The folders agents' file tools are denied: the container tool's, on a
/// Mac where the browser is on (set up or not: no agent's work reads it).
/// The refusal points an agent with the browser's tools (`browser_tools`)
/// to them.
pub(crate) fn denied(browser_tools: bool) -> Vec<factory_agent::tools::Denied> {
    if !(cfg!(target_os = "macos") && super::enabled()) {
        return Vec::new();
    }
    let why = if browser_tools {
        "holds Claktory's browser and its sign-ins, which no agent's tools can read or change. Use the browser tools instead."
    } else {
        "holds Claktory's browser and its sign-ins, which no agent's tools can read or change."
    };
    data_root().into_iter().map(|dir| factory_agent::tools::Denied { dir, why: Some(why.to_owned()) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_profile_denies_only_the_container_tool_and_its_folder_and_quotes_it() {
        let profile = profile(Path::new("/Users/a b/Library/Application Support/com.apple.container\") (allow default"));
        assert!(profile.contains("(allow default)\n"), "{profile}");
        assert!(profile.contains("(deny mach-lookup (global-name-prefix \"com.apple.container.\"))"), "{profile}");
        assert!(
            profile.contains("(subpath \"/Users/a b/Library/Application Support/com.apple.container\\\") (allow default\")"),
            "{profile}"
        );
        assert!(profile.contains("(path-regex \"^/Users/a b/Library/Application Support/com\\\\.apple\\\\.container"), "{profile}");
    }

    #[test]
    fn it_applies_once_a_browser_volume_is_there() {
        let root = std::env::temp_dir().join(format!("claktory-confine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("volumes/someone-else")).unwrap();
        assert!(prefix(&root).is_none(), "no browser here yet");
        std::fs::create_dir_all(root.join("volumes/claktory-browser-0011223344556677")).unwrap();
        if Path::new(SANDBOX_EXEC).is_file() {
            let prefix = prefix(&root).expect("under the profile");
            assert_eq!(&prefix[..2], [OsString::from(SANDBOX_EXEC), OsString::from("-p")]);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The profile does what it says on this Mac: the folder and the
    /// container tool's services are out of reach, everything else works.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_command_under_it_reads_nothing_of_the_folder_and_runs_otherwise() {
        let root = std::env::temp_dir().join(format!("claktory-confine-run-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("volumes/claktory-browser-test")).unwrap();
        std::fs::write(root.join("volumes/claktory-browser-test/volume.img"), "cookies").unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let run = |script: &str| {
            std::process::Command::new(SANDBOX_EXEC).args(["-p", &profile(&root), "/bin/bash", "-c", script]).output().expect("ran")
        };
        let read = run(&format!("cat '{}/volumes/claktory-browser-test/volume.img'", root.display()));
        assert!(!read.status.success() && !String::from_utf8_lossy(&read.stdout).contains("cookies"), "{read:?}");
        let listed = run(&format!("ls '{}'", root.display()));
        assert!(!listed.status.success(), "{listed:?}");
        let fine = run("echo fine > /dev/null && /usr/bin/true");
        assert!(fine.status.success(), "{fine:?}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
