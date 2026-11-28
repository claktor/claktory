//! The private Git for Windows a release carries, in the home's `tools`.
//!
//! A release names its Git in `tools\git.txt` (such as `git-2.51.0`) and
//! carries it in `tools\<that>\`. An update copies a Git the home doesn't
//! have yet in beside the one in use, then points `tools\git.txt` at it,
//! keeping what it named before in `tools\.previous\git.txt` for a
//! roll-back. Agents' commands may be running from the Git in use, which
//! Windows won't let anything rename or delete, so it's never moved: each
//! Git has its own folder, and those named by neither pointer are deleted
//! at the next start, once nothing runs from them.

use std::path::{Path, PathBuf};

use factory_agent::windows::shell::{GIT_POINTER, is_git_folder};

use crate::home::Home;

fn tools(home: &Home) -> PathBuf {
    crate::update::tools_dir(home)
}

fn previous_pointer(home: &Home) -> PathBuf {
    tools(home).join(".previous").join(GIT_POINTER)
}

/// The Git folder a pointer file names, if it names one.
fn named(pointer: &Path) -> Option<String> {
    let text = std::fs::read_to_string(pointer).ok()?;
    let name = text.trim();
    is_git_folder(name).then(|| name.to_owned())
}

/// Writes a pointer: a temporary file, then moved over the old one.
fn point(pointer: &Path, name: &str) -> std::io::Result<()> {
    if let Some(folder) = pointer.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let staged = pointer.with_extension("new");
    std::fs::write(&staged, format!("{name}\n"))?;
    std::fs::rename(&staged, pointer)
}

fn copy_folder(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_folder(&entry.path(), &to.join(entry.file_name()))?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        } else {
            return Err(std::io::Error::other(format!("{} is a link", entry.path().display())));
        }
    }
    Ok(())
}

/// Puts the Git `release` names in the home's `tools`, unless it's there
/// already; its name.
fn put_in(home: &Home, release: &Path) -> std::io::Result<Option<String>> {
    let Some(name) = named(&release.join("tools").join(GIT_POINTER)) else { return Ok(None) };
    let from = release.join("tools").join(&name);
    if !from.is_dir() {
        return Err(std::io::Error::other(format!("the release names {name} but doesn't carry it")));
    }
    let to = tools(home).join(&name);
    if !to.is_dir() {
        let partial = tools(home).join(format!(".{name}.partial"));
        let _ = std::fs::remove_dir_all(&partial);
        copy_folder(&from, &partial)?;
        std::fs::rename(&partial, &to)?;
    }
    Ok(Some(name))
}

/// Before an update's backend goes in: the release's Git, pointed at, and
/// the one before kept named for a roll-back. A release without one leaves
/// the Git in use alone, and forgets an older previous one.
pub fn stage(home: &Home, release: &Path) -> std::io::Result<()> {
    let Some(name) = put_in(home, release)? else {
        return match std::fs::remove_file(previous_pointer(home)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    };
    let pointer = tools(home).join(GIT_POINTER);
    match named(&pointer) {
        Some(before) => point(&previous_pointer(home), &before)?,
        None => {
            let _ = std::fs::remove_file(previous_pointer(home));
        }
    }
    point(&pointer, &name)
}

/// When an update is undone: the Git named before it, named again.
pub fn restore(home: &Home) -> std::io::Result<()> {
    match named(&previous_pointer(home)) {
        Some(before) => point(&tools(home).join(GIT_POINTER), &before),
        None => Ok(()),
    }
}

/// The Git `release` carries, put in and pointed at, when the home names
/// none yet: at start, and as `factoryd install` sets a home up.
pub fn put_in_unless_named(home: &Home, release: &Path) -> std::io::Result<()> {
    if named(&tools(home).join(GIT_POINTER)).is_some() {
        return Ok(());
    }
    match put_in(home, release)? {
        Some(name) => point(&tools(home).join(GIT_POINTER), &name),
        None => Ok(()),
    }
}

/// At start: the running release's Git, when the home has none named (an
/// update from a backend that knew of none); then every Git folder named
/// by neither pointer deleted, skipping one still in use.
pub fn heal_and_sweep(home: &Home, release: &Path) {
    if let Err(error) = put_in_unless_named(home, release) {
        tracing::error!(%error, "could not put in the release's Git");
    }
    let kept = [named(&tools(home).join(GIT_POINTER)), named(&previous_pointer(home))];
    let Ok(entries) = std::fs::read_dir(tools(home)) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let old = name.starts_with("git-") && is_git_folder(&name) && !kept.iter().flatten().any(|kept| *kept == name);
        let stale = name.starts_with(".git-") && name.ends_with(".partial");
        if (old || stale) && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_with(dir: &Path, name: &str) -> PathBuf {
        let release = dir.join(format!("release-{name}"));
        let bash = release.join("tools").join(name).join("usr").join("bin");
        std::fs::create_dir_all(&bash).expect("release");
        std::fs::write(bash.join("bash.exe"), name).expect("bash");
        std::fs::write(release.join("tools").join(GIT_POINTER), format!("{name}\r\n")).expect("pointer");
        release
    }

    fn in_use(home: &Home) -> Option<String> {
        named(&tools(home).join(GIT_POINTER))
    }

    #[test]
    fn an_update_puts_its_git_beside_the_one_in_use_and_a_roll_back_points_back() {
        let dir = std::env::temp_dir().join(format!("claktory-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = Home::new(dir.join("home"));
        let first = release_with(&dir, "git-2.50.1");
        heal_and_sweep(&home, &first);
        assert_eq!(in_use(&home).as_deref(), Some("git-2.50.1"));
        assert!(tools(&home).join("git-2.50.1/usr/bin/bash.exe").is_file());

        let second = release_with(&dir, "git-2.51.0");
        stage(&home, &second).expect("staged");
        assert_eq!(in_use(&home).as_deref(), Some("git-2.51.0"));
        assert!(tools(&home).join("git-2.50.1").is_dir(), "the one in use before stays");
        restore(&home).expect("restored");
        assert_eq!(in_use(&home).as_deref(), Some("git-2.50.1"));

        // Staged again, then kept: the one before is swept only once
        // neither pointer names it.
        stage(&home, &second).expect("staged");
        heal_and_sweep(&home, &second);
        assert!(tools(&home).join("git-2.50.1").is_dir(), "named for a roll-back");
        let third = release_with(&dir, "git-2.52.0");
        stage(&home, &third).expect("staged");
        heal_and_sweep(&home, &third);
        assert!(!tools(&home).join("git-2.50.1").exists());
        assert!(tools(&home).join("git-2.51.0").is_dir() && tools(&home).join("git-2.52.0").is_dir());

        // A release that names a Git it doesn't carry fails the update.
        let broken = dir.join("broken");
        std::fs::create_dir_all(broken.join("tools")).expect("broken");
        std::fs::write(broken.join("tools").join(GIT_POINTER), "git-9.9.9").expect("pointer");
        assert!(stage(&home, &broken).is_err());
        assert_eq!(in_use(&home).as_deref(), Some("git-2.52.0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pointer_naming_anything_but_a_git_folder_is_ignored() {
        let dir = std::env::temp_dir().join(format!("claktory-git-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let home = Home::new(dir.join("home"));
        let release = dir.join("release");
        std::fs::create_dir_all(release.join("tools").join("..").join("bin")).expect("release");
        std::fs::write(release.join("tools").join(GIT_POINTER), "..").expect("pointer");
        stage(&home, &release).expect("nothing to do");
        assert_eq!(in_use(&home), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
