//! Where the moved Claktory's paths are on this computer: a list of
//! prefixes, the source's on the left, and what each path under one becomes.

use std::path::{Component, Path, PathBuf};

/// Prefixes the source's paths are moved by, longest first, so the most
/// specific one wins: `/Users/me/factory=/srv/factory` over
/// `/Users/me=/home/you`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PathMap {
    pairs: Vec<(PathBuf, PathBuf)>,
}

impl PathMap {
    /// The map from `pairs`; a later pair for the same prefix replaces an
    /// earlier one, so what the person gives wins over the defaults.
    pub(crate) fn new(pairs: impl IntoIterator<Item = (PathBuf, PathBuf)>) -> PathMap {
        let mut map = PathMap::default();
        for (from, to) in pairs {
            let (from, to) = (clean(&from), clean(&to));
            map.pairs.retain(|(known, _)| *known != from);
            map.pairs.push((from, to));
        }
        map.pairs.sort_by(|a, b| b.0.as_os_str().len().cmp(&a.0.as_os_str().len()).then_with(|| a.0.cmp(&b.0)));
        map
    }

    pub(crate) fn pairs(&self) -> &[(PathBuf, PathBuf)] {
        &self.pairs
    }

    /// Where `path` is here; None when no prefix holds it.
    pub(crate) fn path(&self, path: &Path) -> Option<PathBuf> {
        self.pairs.iter().find_map(|(from, to)| {
            let rest = path.strip_prefix(from).ok()?;
            Some(if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) })
        })
    }

    /// `path` here, or as it was when no prefix holds it.
    pub(crate) fn path_or_same(&self, path: &Path) -> PathBuf {
        self.path(path).unwrap_or_else(|| path.to_path_buf())
    }

    /// `text` (a command, say) with each path in it that starts with a
    /// prefix moved: a prefix counts where it's followed by `/` or by
    /// something that can't be part of a file name in a command (a space, a
    /// quote, the end), so `/Users/me` doesn't move `/Users/mex`.
    pub(crate) fn text(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        'scan: while !rest.is_empty() {
            for (from, to) in &self.pairs {
                let from = from.to_string_lossy();
                if let Some(after) = rest.strip_prefix(&*from)
                    && after.chars().next().is_none_or(|next| !(next.is_alphanumeric() || matches!(next, '.' | '_' | '-' | '~')))
                {
                    out.push_str(&to.to_string_lossy());
                    rest = after;
                    continue 'scan;
                }
            }
            let mut chars = rest.chars();
            if let Some(next) = chars.next() {
                out.push(next);
            }
            rest = chars.as_str();
        }
        out
    }

    /// Each pair, as the agents' event says it: "/Users/me → /home/you".
    pub(crate) fn said(&self) -> String {
        let mut pairs: Vec<&(PathBuf, PathBuf)> = self.pairs.iter().filter(|(from, to)| from != to).collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs.iter().map(|(from, to)| format!("{} → {}", from.display(), to.display())).collect::<Vec<_>>().join("; ")
    }
}

/// `path` without a trailing `/`, `.` or `..` parts (left as they are when
/// they'd climb out of the root).
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// One `--map FROM=TO`: both absolute.
pub fn parse_pair(text: &str) -> Result<(PathBuf, PathBuf), String> {
    let (from, to) = text.split_once('=').ok_or_else(|| format!("--map takes FROM=TO, such as /Users/me=/home/you; got \"{text}\""))?;
    let (from, to) = (PathBuf::from(from.trim()), PathBuf::from(to.trim()));
    if !from.is_absolute() || !to.is_absolute() {
        return Err(format!("--map takes absolute paths on both sides; got \"{text}\""));
    }
    Ok((from, to))
}

// Moving isn't on Windows yet (`move_run::NOT_ON_WINDOWS`).
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn map() -> PathMap {
        PathMap::new([
            (PathBuf::from("/Users/me"), PathBuf::from("/home/you")),
            (PathBuf::from("/Users/me/factory/"), PathBuf::from("/srv/factory")),
        ])
    }

    #[test]
    fn the_longest_prefix_wins_and_only_whole_names_move() {
        let map = map();
        assert_eq!(map.path(Path::new("/Users/me/factory/app")), Some(PathBuf::from("/srv/factory/app")));
        assert_eq!(map.path(Path::new("/Users/me/.factory")), Some(PathBuf::from("/home/you/.factory")));
        assert_eq!(map.path(Path::new("/Users/me")), Some(PathBuf::from("/home/you")));
        assert_eq!(map.path(Path::new("/Users/mex/x")), None);
        assert_eq!(map.path(Path::new("/opt/tools")), None);
        assert_eq!(
            map.text("cd /Users/me/factory && cargo test > '/Users/me/log.txt'; ls /Users/mex"),
            "cd /srv/factory && cargo test > '/home/you/log.txt'; ls /Users/mex"
        );
        assert_eq!(map.said(), "/Users/me → /home/you; /Users/me/factory → /srv/factory");
        // The person's pair replaces a default for the same prefix.
        let mine =
            PathMap::new([(PathBuf::from("/Users/me"), PathBuf::from("/home/you")), (PathBuf::from("/Users/me"), PathBuf::from("/data"))]);
        assert_eq!(mine.path(Path::new("/Users/me/x")), Some(PathBuf::from("/data/x")));
        assert!(parse_pair("/a=/b").is_ok() && parse_pair("a=/b").is_err() && parse_pair("/a").is_err());
    }
}
