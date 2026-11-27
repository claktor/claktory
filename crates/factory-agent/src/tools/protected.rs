//! macOS's protected places (permissions.md P2, item 12): Documents,
//! Desktop, Downloads, iCloud Drive and removable or network volumes, which
//! macOS keeps from an app until the person lets it in (Files and Folders).
//! A refusal there comes back as "Operation not permitted" (EPERM), which
//! the agent would otherwise read as a bug to work around. Recognised for
//! sure in the file tools and by its words in bash's output, it's said for
//! what it is, and the result's details carry the place
//! (`"protected_refused"`: its root, absolute), which factoryd reads to show
//! the person the folder refused (it checks the root with `protected_root`
//! again before it believes it).

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::errors::Error;
use crate::types::{ToolResult, ToolResultPart};

/// The details key a refused result carries.
pub const DETAILS_KEY: &str = "protected_refused";

/// The protected place `path` is in, as its root ("/Users/ana/Documents",
/// "/Volumes/Backup"); None outside them, and on any system but macOS.
pub fn protected_root(path: &Path) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    protected_root_in(&home, path)
}

/// `protected_root` with the home folder given: on every system, for tests.
pub fn protected_root_in(home: &Path, path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() || path.components().any(|part| matches!(part, Component::ParentDir)) {
        return None;
    }
    for place in ["Documents", "Desktop", "Downloads", "Library/Mobile Documents"] {
        let root = home.join(place);
        if path.starts_with(&root) {
            return Some(root);
        }
    }
    let mut parts = path.components();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Component::RootDir), Some(Component::Normal(volumes)), Some(Component::Normal(name))) if volumes == "Volumes" => {
            Some(Path::new("/Volumes").join(name))
        }
        _ => None,
    }
}

/// The place as the person knows it: "Documents", "iCloud Drive", or a
/// volume's name.
pub fn place_name(root: &Path) -> String {
    if root.ends_with("Library/Mobile Documents") {
        return "iCloud Drive".to_owned();
    }
    root.file_name().map_or_else(|| root.display().to_string(), |name| name.to_string_lossy().into_owned())
}

/// What the agent reads (permissions.md P2).
fn said(root: &Path) -> String {
    format!(
        "macOS hasn't allowed Claktory Agents into {}, so this failed. The person is shown how to allow it in Needs you; don't work around it.",
        place_name(root)
    )
}

/// A file tool's error that is macOS refusing a protected place: the
/// result to give instead, saying so, with the place in its details.
pub fn refused(error: &Error) -> Option<ToolResult> {
    let Error::Fs(fs) = error else { return None };
    if fs.code != "EPERM" {
        return None;
    }
    let root = [fs.path.as_deref(), fs.dest.as_deref()].into_iter().flatten().find_map(protected_root)?;
    Some(result(format!("{error}\n{}", said(&root)), &root))
}

fn result(text: String, root: &Path) -> ToolResult {
    let mut details = serde_json::Map::new();
    details.insert(DETAILS_KEY.to_owned(), Value::String(root.display().to_string()));
    ToolResult::text(text, Value::Object(details))
}

/// Best effort for bash: when its output says "Operation not permitted" of
/// a path in a protected place (or of one it names relative to `cwd`, or
/// with no path while it ran in one), the result says what it is and
/// carries the place. Anything else is left as it is.
pub fn noted(mut result: ToolResult, cwd: &Path) -> ToolResult {
    let Some(root) = refused_in(&text_of(&result), cwd, protected_root) else { return result };
    if let Value::Object(details) = &mut result.details {
        details.insert(DETAILS_KEY.to_owned(), Value::String(root.display().to_string()));
    }
    let line = format!("\n{}", said(&root));
    match result.content.iter_mut().rev().find_map(|part| match part {
        ToolResultPart::Text { text } => Some(text),
        _ => None,
    }) {
        Some(text) => text.push_str(&line),
        None => result.content.push(ToolResultPart::Text { text: line }),
    }
    result
}

/// A result's text, its parts joined.
fn text_of(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|part| match part {
            ToolResultPart::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The protected place bash's `output` was refused in, by its words.
pub fn refused_in(output: &str, cwd: &Path, root_of: impl Fn(&Path) -> Option<PathBuf>) -> Option<PathBuf> {
    let mut in_cwd = None;
    for line in output.lines().filter(|line| line.to_ascii_lowercase().contains("operation not permitted")) {
        // A line naming a place elsewhere isn't about where it ran.
        let mut elsewhere = false;
        for word in line.split_whitespace() {
            let word = word.trim_matches(|c: char| matches!(c, '\'' | '"' | ':' | ',' | '`' | '(' | ')' | '[' | ']'));
            if word.is_empty() || word.contains("://") {
                continue;
            }
            let path = match word.strip_prefix("~/") {
                Some(rest) => match std::env::var_os("HOME") {
                    Some(home) => PathBuf::from(home).join(rest),
                    None => continue,
                },
                None if word.starts_with('/') => PathBuf::from(word),
                None if word.contains('/') || word.contains('.') => crate::tools::fs::resolve(cwd, word),
                None => continue,
            };
            match root_of(&path) {
                Some(root) => return Some(root),
                None => elsewhere |= word.starts_with('/') || word.starts_with("~/"),
            }
        }
        if !elsewhere {
            in_cwd = in_cwd.or_else(|| root_of(cwd));
        }
    }
    in_cwd
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/ana";

    fn root(path: &str) -> Option<PathBuf> {
        protected_root_in(Path::new(HOME), Path::new(path))
    }

    #[test]
    fn the_protected_places_are_known_by_their_roots() {
        assert_eq!(root("/Users/ana/Documents/site/a.md"), Some(PathBuf::from("/Users/ana/Documents")));
        assert_eq!(root("/Users/ana/Desktop"), Some(PathBuf::from("/Users/ana/Desktop")));
        assert_eq!(root("/Users/ana/Library/Mobile Documents/com~apple~CloudDocs/x"), Some(PathBuf::from("/Users/ana/Library/Mobile Documents")));
        assert_eq!(root("/Volumes/Backup/photos"), Some(PathBuf::from("/Volumes/Backup")));
        assert_eq!(root("/Users/ana/code/site"), None);
        assert_eq!(root("/Users/ana/Documentsx/a"), None);
        assert_eq!(root("/Users/ana/Documents/../code"), None);
        assert_eq!(root("/Volumes"), None);
        assert_eq!(place_name(Path::new("/Users/ana/Library/Mobile Documents")), "iCloud Drive");
        assert_eq!(place_name(Path::new("/Volumes/Backup")), "Backup");
        assert_eq!(place_name(Path::new("/Users/ana/Downloads")), "Downloads");
    }

    #[test]
    fn bash_output_is_read_for_a_refusal_in_a_protected_place() {
        let cwd = Path::new("/Users/ana/code");
        let found = |output: &str, cwd: &Path| refused_in(output, cwd, |path| protected_root_in(Path::new(HOME), path));
        assert_eq!(found("cat: /Users/ana/Documents/notes.txt: Operation not permitted\n", cwd), Some(PathBuf::from("/Users/ana/Documents")));
        assert_eq!(
            found("PermissionError: [Errno 1] Operation not permitted: '/Volumes/Backup/x'", cwd),
            Some(PathBuf::from("/Volumes/Backup"))
        );
        // Relative to where it ran, or with no path while it ran there.
        let in_desktop = Path::new("/Users/ana/Desktop/site");
        assert_eq!(found("ls: ./src: Operation not permitted", in_desktop), Some(PathBuf::from("/Users/ana/Desktop")));
        assert_eq!(found("zsh: operation not permitted", in_desktop), Some(PathBuf::from("/Users/ana/Desktop")));
        // Not a protected place, or not a refusal: nothing, even run in one.
        assert_eq!(found("rm: /usr/bin/x: Operation not permitted", cwd), None);
        assert_eq!(found("rm: /usr/bin/x: Operation not permitted", in_desktop), None);
        assert_eq!(found("cat: /Users/ana/Documents/a: No such file or directory", cwd), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_file_tools_refusal_in_a_protected_place_says_so_with_the_place() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let path = home.join("Documents/site/a.md");
        let error: Error = crate::errors::FsError::new(std::io::Error::from_raw_os_error(libc::EPERM), "open", Some(&path), None).into();
        let result = refused(&error).expect("recognised");
        let text = text_of(&result);
        assert!(text.contains("macOS hasn't allowed Claktory Agents into Documents, so this failed."), "{text}");
        assert!(text.starts_with("EPERM: "), "the error itself first: {text}");
        assert_eq!(result.details[DETAILS_KEY], home.join("Documents").display().to_string());
        // Another error, or elsewhere: left to the usual error.
        let missing: Error = crate::errors::FsError::new(std::io::Error::from_raw_os_error(libc::ENOENT), "open", Some(&path), None).into();
        assert!(refused(&missing).is_none());
        let elsewhere: Error =
            crate::errors::FsError::new(std::io::Error::from_raw_os_error(libc::EPERM), "open", Some(Path::new("/usr/bin/x")), None).into();
        assert!(refused(&elsewhere).is_none());
    }
}
