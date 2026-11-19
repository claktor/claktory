//! Own checkouts: an agent with the "tree" extension on works in a checkout
//! of its project's git repository of its own, on its own branch, so agents
//! working in parallel don't step on each other's edits and builds.
//!
//! A checkout is a git worktree made copy-on-write: Factory registers the
//! worktree without writing any files, clones the files git tracks in the main
//! checkout into it where the filesystem can share blocks (APFS `clonefile`,
//! Linux reflinks), gives it the main checkout's index, then lets git rewrite
//! only the files that differ. Only tracked files: build output, dependencies
//! and anything else git ignores stay out, so making one takes about as long
//! as the repository has files, never as long as its build output (a Rust
//! target/ can hold millions). Where cloning isn't possible it's a plain
//! checkout, which git writes.
//!
//! Checkouts live in `<home>/trees/<agent>/<repository folder name>`, on the
//! branch `<home's name>/<agent>` (`factory/<agent>` for ~/.factory), each with a `<name>.json` beside it saying
//! which repository it belongs to. They persist with the agent; they're
//! removed when it's deleted or the extension is turned off, but only when
//! nothing would be lost: no uncommitted changes, and no commits that aren't
//! in the main checkout's branch.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::home::Home;

/// The folder all checkouts are in.
pub(crate) fn trees_dir(home: &Home) -> PathBuf {
    home.root().join("trees")
}

/// An agent's checkout of the repository a folder is in, made or not.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Checkout {
    /// The checkout's top folder.
    pub(crate) tree: PathBuf,
    /// Where the agent works: the same place in the checkout as `source` is
    /// in the repository.
    pub(crate) folder: PathBuf,
    /// The folder it would work in otherwise.
    pub(crate) source: PathBuf,
    /// The main checkout's top folder.
    pub(crate) repo: PathBuf,
    pub(crate) branch: String,
}

/// What `<tree>.json` keeps about a checkout that was made.
#[derive(Serialize, Deserialize)]
struct Record {
    /// The main checkout's top folder, as it was found.
    repo: PathBuf,
    branch: String,
}

/// Why an agent has no checkout.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Unavailable {
    #[error("{0} isn't in a git repository")]
    NotGit(PathBuf),
    #[error("the git repository at {0} holds Factory's own folder")]
    HoldsFactory(PathBuf),
}

/// The checkout `agent` would have of the repository `source` is in, without
/// making it.
pub(crate) fn plan(home: &Home, agent: &str, source: &Path) -> Result<Checkout, Unavailable> {
    let repo = source.ancestors().find(|dir| dir.join(".git").exists()).ok_or_else(|| Unavailable::NotGit(source.to_path_buf()))?;
    let trees = trees_dir(home);
    // A repository around Factory's home (a home folder kept in git, say)
    // would be cloned into itself.
    if trees.starts_with(repo) || home.root().starts_with(repo) {
        return Err(Unavailable::HoldsFactory(repo.to_path_buf()));
    }
    let name = repo.file_name().map_or_else(|| "repository".into(), |name| name.to_string_lossy().into_owned());
    let tree = trees.join(agent).join(&name);
    let inside = source.strip_prefix(repo).unwrap_or(Path::new(""));
    Ok(Checkout {
        folder: if inside.as_os_str().is_empty() { tree.clone() } else { tree.join(inside) },
        tree,
        source: source.to_path_buf(),
        repo: repo.to_path_buf(),
        branch: format!("{}/{agent}", branch_prefix(home)),
    })
}

/// The first part of a checkout's branch: the Factory's home folder's name,
/// "factory" for ~/.factory and "factory-dev" for ~/.factory-dev, so two
/// Factories working on one repository never take each other's branches.
fn branch_prefix(home: &Home) -> String {
    let name = home.root().file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let name: String = name.trim_start_matches('.').chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    if name.is_empty() { "factory".to_owned() } else { name }
}

impl Checkout {
    fn record_path(&self) -> PathBuf {
        self.tree.with_extension("json")
    }

    /// Whether it was made, for this repository.
    pub(crate) fn exists(&self) -> bool {
        let Ok(text) = std::fs::read_to_string(self.record_path()) else { return false };
        let Ok(record) = serde_json::from_str::<Record>(&text) else { return false };
        record.repo == self.repo && self.tree.join(".git").is_file()
    }
}

/// Checkouts that couldn't be made, by `tree`, and why: they aren't tried
/// again until the extension is turned on again or Factory restarts.
static FAILED: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

pub(crate) fn failure(checkout: &Checkout) -> Option<String> {
    let failed = FAILED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    failed.iter().find(|(tree, _)| *tree == checkout.tree).map(|(_, why)| why.clone())
}

fn set_failure(checkout: &Checkout, why: Option<String>) {
    let mut failed = FAILED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    failed.retain(|(tree, _)| *tree != checkout.tree);
    if let Some(why) = why {
        failed.push((checkout.tree.clone(), why));
    }
}

/// Forgets that `agent`'s checkouts couldn't be made, so they're tried again.
pub(crate) fn retry(home: &Home, agent: &str) {
    let dir = trees_dir(home).join(agent);
    FAILED.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|(tree, _)| !tree.starts_with(&dir));
}

/// How a checkout was made.
#[derive(Debug, PartialEq)]
pub(crate) enum Made {
    /// It was there already.
    Existing,
    /// Its tracked files cloned copy-on-write.
    Cloned,
    /// Written out by git: the filesystem can't clone, or the checkout is on
    /// another volume than the repository.
    Plain,
}

/// Makes the checkout if it isn't there. Blocking, for about as long as the
/// repository has tracked files. A failure is remembered (see `failure`).
pub(crate) fn ensure(checkout: &Checkout) -> Result<Made, String> {
    if checkout.exists() {
        return Ok(Made::Existing);
    }
    let made = make(checkout, true);
    if let Err(error) = &made {
        set_failure(checkout, Some(error.clone()));
        // Whatever was made of it goes, so the next try starts clean.
        let _ = git(&checkout.repo, &["worktree", "remove", "--force", &checkout.tree.to_string_lossy()]);
        let _ = std::fs::remove_dir_all(&checkout.tree);
        let _ = git(&checkout.repo, &["worktree", "prune"]);
    } else {
        set_failure(checkout, None);
    }
    made
}

/// Makes it, cloned where `clone` and the filesystem allow, else plain.
fn make(checkout: &Checkout, clone: bool) -> Result<Made, String> {
    let (repo, tree) = (&checkout.repo, &checkout.tree);
    let base = git(repo, &["rev-parse", "--verify", "-q", "HEAD"]).map_err(|_| format!("{} has no commits yet", repo.display()))?;
    // Left over from an attempt that didn't finish.
    if tree.exists() {
        let _ = git(repo, &["worktree", "remove", "--force", &tree.to_string_lossy()]);
        std::fs::remove_dir_all(tree).or_else(ignore_missing).map_err(|error| format!("Can't clear {}: {error}", tree.display()))?;
    }
    let _ = git(repo, &["worktree", "prune"]);
    let parent = tree.parent().unwrap_or(tree);
    std::fs::create_dir_all(parent).map_err(|error| format!("Can't make {}: {error}", parent.display()))?;
    let path = tree.to_string_lossy();
    // The branch is kept from an earlier checkout that wasn't landed; the
    // work on it carries on.
    let branch_ref = format!("refs/heads/{}", checkout.branch);
    let (start, mut args) = match git(repo, &["rev-parse", "--verify", "-q", &branch_ref]) {
        Ok(tip) => (tip, vec!["worktree", "add", "--no-checkout", &path, &checkout.branch]),
        Err(_) => (base, vec!["worktree", "add", "--no-checkout", "-b", &checkout.branch, &path, "HEAD"]),
    };
    args.insert(0, "-c");
    args.insert(1, "core.hooksPath=/dev/null");
    git(repo, &args)?;
    let made = if clone && same_volume(repo, parent) && clone_tracked(repo, tree).is_ok() && copy_index(repo, tree).is_ok() {
        Made::Cloned
    } else {
        // Nothing cloned stays: git writes every file.
        clear_except_git(tree)?;
        Made::Plain
    };
    // Rewrites what differs from the branch: the main checkout's uncommitted
    // changes, or the branch's own commits. The clones are new files with the
    // originals' times and sizes, which is all a minimal stat check compares,
    // so git trusts them instead of writing every file again.
    git(tree, &["-c", "core.checkStat=minimal", "reset", "-q", "--hard", &start])?;
    let record = Record { repo: repo.clone(), branch: checkout.branch.clone() };
    let text = serde_json::to_string_pretty(&record).map_err(|error| error.to_string())?;
    std::fs::write(checkout.record_path(), text).map_err(|error| format!("Can't write {}: {error}", checkout.record_path().display()))?;
    Ok(made)
}

fn ignore_missing(error: std::io::Error) -> std::io::Result<()> {
    if error.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(error) }
}

fn same_volume(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev(),
        _ => false,
    }
}

/// Clones the files git tracks in the main checkout into the checkout, as
/// they are there, a few threads at a time. A tracked file that's missing
/// there (deleted, not committed) or a submodule is left to git.
fn clone_tracked(repo: &Path, tree: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let listed = Command::new("git").arg("-C").arg(repo).args(["ls-files", "-z"]).output()?;
    if !listed.status.success() {
        return Err(std::io::Error::other("git ls-files failed"));
    }
    let files: Vec<&Path> = listed
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| Path::new(std::ffi::OsStr::from_bytes(path)))
        .collect();
    let mut folders: Vec<&Path> = files.iter().filter_map(|file| file.parent()).filter(|folder| !folder.as_os_str().is_empty()).collect();
    folders.sort_unstable();
    folders.dedup();
    for folder in folders {
        std::fs::create_dir_all(tree.join(folder))?;
    }
    let threads = std::thread::available_parallelism().map_or(4, std::num::NonZero::get).min(8);
    std::thread::scope(|scope| {
        let workers: Vec<_> = files
            .chunks(files.len().div_ceil(threads).max(1))
            .map(|chunk| {
                scope.spawn(move || {
                    for file in chunk {
                        let from = repo.join(file);
                        match std::fs::symlink_metadata(&from) {
                            Ok(meta) if meta.is_dir() => continue,
                            Ok(_) => clone(&from, &tree.join(file))?,
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(error) => return Err(error),
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        workers.into_iter().try_for_each(|worker| worker.join().unwrap_or_else(|_| Err(std::io::Error::other("a clone thread panicked"))))
    })
}

/// Clones a file or symbolic link with `clonefile`, sharing its blocks.
#[cfg(target_os = "macos")]
fn clone(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn clonefile(src: *const std::ffi::c_char, dst: *const std::ffi::c_char, flags: u32) -> std::ffi::c_int;
    }
    /// Copy symbolic links themselves, not what they point to.
    const CLONE_NOFOLLOW: u32 = 1;
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both are valid NUL-terminated paths for the call's duration.
    let result = unsafe { clonefile(from.as_ptr(), to.as_ptr(), CLONE_NOFOLLOW) };
    if result == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
}

/// Clones a file with a reflink (`FICLONE`), where the filesystem has them
/// (btrfs, XFS…); a symbolic link is made again.
#[cfg(not(target_os = "macos"))]
fn clone(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::io::AsRawFd;
    /// `_IOW(0x94, 9, int)`, Linux's FICLONE.
    const FICLONE: libc::c_ulong = 0x4004_9409;
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        return std::os::unix::fs::symlink(std::fs::read_link(from)?, to);
    }
    let source = std::fs::File::open(from)?;
    let target = std::fs::File::create(to)?;
    // SAFETY: both descriptors are open for the call's duration.
    if unsafe { libc::ioctl(target.as_raw_fd(), FICLONE as _, source.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    target.set_permissions(std::fs::Permissions::from_mode(meta.permissions().mode()))
}

/// Gives the checkout the main checkout's index, so git sees the cloned
/// files as what they are and rewrites only those that differ.
fn copy_index(repo: &Path, tree: &Path) -> Result<(), String> {
    let from = git_path(repo, "index")?;
    let to = git_path(tree, "index")?;
    std::fs::copy(&from, &to).map(|_| ()).map_err(|error| format!("Can't copy {}: {error}", from.display()))
}

/// A path in a checkout's git folder (`git rev-parse --git-path`).
fn git_path(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(git(dir, &["rev-parse", "--git-path", name])?);
    Ok(if path.is_absolute() { path } else { dir.join(path) })
}

fn clear_except_git(tree: &Path) -> Result<(), String> {
    let entries = std::fs::read_dir(tree).map_err(|error| format!("Can't read {}: {error}", tree.display()))?;
    for entry in entries.flatten() {
        if entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let removed =
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        removed.map_err(|error| format!("Can't remove {}: {error}", path.display()))?;
    }
    Ok(())
}

/// Runs git in `dir`; its output, trimmed, or what it said went wrong.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|error| format!("Can't run git: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(if said.is_empty() { format!("git {} failed", args.join(" ")) } else { said })
    }
}

/// The main checkout's branch, such as "main"; None when it's detached.
pub(crate) fn main_branch(repo: &Path) -> Option<String> {
    git(repo, &["symbolic-ref", "--short", "-q", "HEAD"]).ok().filter(|branch| !branch.is_empty())
}

/// Lands the checkout's branch: fast-forwards the main checkout's branch to
/// it, in the main checkout. Says what happened, or why it can't.
pub(crate) fn land(checkout: &Checkout) -> Result<String, String> {
    if !checkout.exists() {
        return Err("You have no checkout of your own yet.".to_owned());
    }
    let Some(main) = main_branch(&checkout.repo) else {
        return Err(format!("The main checkout, {}, isn't on a branch, so there's nothing to land on.", checkout.repo.display()));
    };
    let tip = git(&checkout.tree, &["rev-parse", "HEAD"])?;
    let before = git(&checkout.repo, &["rev-parse", "HEAD"])?;
    if tip == before {
        return Ok(format!("Nothing to land: {main} is already at your branch's tip."));
    }
    if git(&checkout.repo, &["merge-base", "--is-ancestor", &tip, &before]).is_ok() {
        return Ok(format!("Nothing to land: your branch's commits are already in {main}."));
    }
    if git(&checkout.repo, &["merge-base", "--is-ancestor", &before, &tip]).is_err() {
        return Err(format!(
            "{main} has moved on since your branch left it. Rebase onto it first (git rebase {main}), test, then land again."
        ));
    }
    let count = git(&checkout.repo, &["rev-list", "--count", &format!("{before}..{tip}")])?;
    git(&checkout.repo, &["merge", "--ff-only", "-q", &tip]).map_err(|said| format!("git refused to fast-forward {main}: {said}"))?;
    let s = if count == "1" { "" } else { "s" };
    Ok(format!("Landed {count} commit{s} on {main} in {}: {}..{}.", checkout.repo.display(), short(&before), short(&tip)))
}

fn short(sha: &str) -> &str {
    sha.get(..10).unwrap_or(sha)
}

/// Why a checkout can't go without losing work; None when it can.
fn keeps_work(checkout: &Checkout) -> Option<String> {
    match git(&checkout.tree, &["status", "--porcelain"]) {
        Ok(status) if !status.is_empty() => return Some("it has uncommitted changes".to_owned()),
        Ok(_) => {}
        Err(said) => return Some(format!("git can't read it ({said})")),
    }
    let tip = git(&checkout.tree, &["rev-parse", "HEAD"]).ok()?;
    let landed = git(&checkout.repo, &["merge-base", "--is-ancestor", &tip, "HEAD"]).is_ok();
    (!landed).then(|| "it has commits that aren't in the main checkout's branch".to_owned())
}

/// Removes the checkout and its branch if nothing would be lost. Ok(None)
/// when it went (or wasn't there); Ok(Some(why)) when it's kept.
pub(crate) fn remove(checkout: &Checkout) -> Result<Option<String>, String> {
    if !checkout.exists() {
        return Ok(None);
    }
    if let Some(why) = keeps_work(checkout) {
        return Ok(Some(why));
    }
    let path = checkout.tree.to_string_lossy();
    git(&checkout.repo, &["worktree", "remove", "--force", &path])?;
    let _ = git(&checkout.repo, &["branch", "-q", "-D", &checkout.branch]);
    let _ = std::fs::remove_dir_all(&checkout.tree);
    let _ = std::fs::remove_file(checkout.record_path());
    Ok(None)
}

/// Every checkout `agent` has, of any repository.
fn all_of(home: &Home, agent: &str) -> Vec<Checkout> {
    let dir = trees_dir(home).join(agent);
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut checkouts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Some(record) = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str::<Record>(&text).ok()) else {
            continue;
        };
        let tree = path.with_extension("");
        checkouts.push(Checkout { folder: tree.clone(), tree, source: record.repo.clone(), repo: record.repo, branch: record.branch });
    }
    checkouts
}

/// Removes `agent`'s checkouts that can go without losing work; those kept
/// are logged, with why. Blocking.
pub(crate) fn retire(home: &Home, agent: &str) {
    for checkout in all_of(home, agent) {
        match remove(&checkout) {
            Ok(None) => tracing::info!(agent, tree = %checkout.tree.display(), "removed an own checkout"),
            Ok(Some(why)) => {
                tracing::warn!(agent, tree = %checkout.tree.display(), branch = %checkout.branch, why, "kept an own checkout")
            }
            Err(error) => tracing::error!(agent, tree = %checkout.tree.display(), %error, "could not remove an own checkout"),
        }
    }
    let _ = std::fs::remove_dir(trees_dir(home).join(agent));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkout_is_planned_for_the_repository_around_the_folder() {
        let home = Home::new(PathBuf::from("/h/.factory"));
        let planned = plan(&home, "builder", Path::new("/nope/surely-not-a-repo")).unwrap_err();
        assert!(matches!(planned, Unavailable::NotGit(_)));
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let checkout = plan(&home, "builder", here).expect("factoryd is in a repository");
        let repo = here.ancestors().nth(2).expect("the repository");
        assert_eq!(checkout.repo, repo);
        assert_eq!(checkout.tree, Path::new("/h/.factory/trees/builder").join(repo.file_name().expect("name")));
        assert_eq!(checkout.folder, checkout.tree.join("crates/factoryd"));
        assert_eq!(checkout.branch, "factory/builder");
        let dev = plan(&Home::new(PathBuf::from("/h/.factory-dev")), "builder", here).expect("plan");
        assert_eq!(dev.branch, "factory-dev/builder", "another Factory's branches are its own");
    }

    #[test]
    fn a_repository_around_factory_gets_no_checkout() {
        let dir = temp();
        let home = Home::new(dir.join("home/.factory"));
        std::fs::create_dir_all(dir.join("home")).expect("home");
        run(&dir.join("home"), &["git", "init", "-q"]);
        let planned = plan(&home, "builder", &dir.join("home/.factory/workspace/site"));
        assert!(matches!(planned, Err(Unavailable::HoldsFactory(_))), "{planned:?}");
    }

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("factoryd-trees-{}-{next}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn run(dir: &Path, command: &[&str]) -> String {
        let output = Command::new(command[0]).args(&command[1..]).current_dir(dir).output().expect("run");
        assert!(output.status.success(), "{command:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).expect("write");
        run(dir, &["git", "add", file]);
        run(dir, &["git", "-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "-m", file]);
    }

    /// A repository on main with a commit, an ignored build folder, an
    /// uncommitted change and an untracked file, and a Factory home beside it.
    fn repository() -> (Home, PathBuf) {
        let dir = temp();
        let repo = dir.join("site");
        std::fs::create_dir_all(&repo).expect("repo");
        run(&repo, &["git", "init", "-q", "-b", "main"]);
        std::fs::write(repo.join(".gitignore"), "target/\n").expect("ignore");
        run(&repo, &["git", "add", ".gitignore"]);
        commit(&repo, "lib.rs", "one\n");
        std::fs::create_dir_all(repo.join("src/deep")).expect("src");
        commit(&repo, "src/deep/mod.rs", "nested\n");
        std::os::unix::fs::symlink("lib.rs", repo.join("link.rs")).expect("symlink");
        run(&repo, &["git", "add", "link.rs"]);
        run(&repo, &["git", "-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "-m", "link"]);
        std::fs::create_dir_all(repo.join("target/debug")).expect("target");
        std::fs::write(repo.join("target/debug/built"), "warm").expect("built");
        std::fs::write(repo.join("lib.rs"), "changed, not committed\n").expect("dirty");
        std::fs::write(repo.join("scratch.txt"), "untracked").expect("untracked");
        (Home::new(dir.join("home")), repo)
    }

    #[test]
    fn a_checkout_is_the_branch_clean_with_only_tracked_files_and_made_once() {
        let (home, repo) = repository();
        let checkout = plan(&home, "builder", &repo).expect("plan");
        assert!(!checkout.exists());
        let made = ensure(&checkout).expect("made");
        // APFS clones; elsewhere it depends on the filesystem.
        assert!(made == Made::Cloned || !cfg!(target_os = "macos"), "{made:?}");
        assert!(checkout.exists());
        let tree = &checkout.tree;
        assert_eq!(run(tree, &["git", "branch", "--show-current"]), checkout.branch);
        assert_eq!(run(tree, &["git", "status", "--porcelain"]), "", "clean: the main checkout's own changes stay there");
        assert_eq!(std::fs::read_to_string(tree.join("lib.rs")).expect("lib"), "one\n");
        assert!(!tree.join("scratch.txt").exists());
        assert!(!tree.join("target").exists(), "no build output: what git ignores stays out");
        assert_eq!(std::fs::read_to_string(tree.join("src/deep/mod.rs")).expect("nested"), "nested\n");
        assert!(std::fs::symlink_metadata(tree.join("link.rs")).expect("link").file_type().is_symlink());
        assert_eq!(ensure(&checkout).expect("again"), Made::Existing);
        // The main checkout is as it was.
        assert_eq!(std::fs::read_to_string(repo.join("lib.rs")).expect("lib"), "changed, not committed\n");
        assert_eq!(run(&repo, &["git", "branch", "--show-current"]), "main");
    }

    /// The person's bar: about two seconds, whatever the build output (this
    /// repository's target/ holds millions of files). By hand, since it makes
    /// a checkout of the repository the test runs in, or FACTORY_TIMING_REPO's,
    /// then removes it.
    #[test]
    #[ignore = "makes a checkout of this repository; run by hand"]
    fn a_checkout_of_this_repository_takes_seconds() {
        let home = Home::new(temp().join(".factory-timing"));
        let repo = std::env::var_os("FACTORY_TIMING_REPO").map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
        let checkout = plan(&home, "timing", &repo).expect("plan");
        let started = std::time::Instant::now();
        let made = ensure(&checkout).expect("made");
        let took = started.elapsed();
        let files = run(&checkout.tree, &["git", "ls-files"]).lines().count();
        assert_eq!(remove(&checkout).expect("remove"), None);
        eprintln!("{made:?} checkout of {files} files in {took:?}");
        // Making files costs about as long as there are of them (0.1-0.2 ms
        // each on APFS): a few hundred take well under a second.
        if files < 5_000 {
            assert!(took < std::time::Duration::from_secs(2), "{took:?}");
        }
    }

    #[test]
    fn where_it_cannot_clone_the_checkout_is_a_plain_one() {
        let (home, repo) = repository();
        let checkout = plan(&home, "builder", &repo).expect("plan");
        assert_eq!(make(&checkout, false).expect("made"), Made::Plain);
        assert!(checkout.exists());
        assert_eq!(run(&checkout.tree, &["git", "status", "--porcelain"]), "");
        assert_eq!(std::fs::read_to_string(checkout.tree.join("lib.rs")).expect("lib"), "one\n");
        assert!(!checkout.tree.join("target").exists(), "no build output: git wrote only what it tracks");
    }

    #[test]
    fn work_is_landed_by_fast_forward_and_a_checkout_goes_only_when_nothing_is_lost() {
        let (home, repo) = repository();
        // Landing needs a main checkout whose changes don't collide.
        std::fs::write(repo.join("lib.rs"), "one\n").expect("clean");
        let checkout = plan(&home, "builder", &repo).expect("plan");
        ensure(&checkout).expect("made");
        let tree = checkout.tree.clone();

        std::fs::write(tree.join("new.rs"), "work").expect("work");
        assert_eq!(remove(&checkout).expect("remove"), Some("it has uncommitted changes".to_owned()));
        run(&tree, &["git", "add", "new.rs"]);
        run(&tree, &["git", "-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "-m", "work"]);
        assert_eq!(remove(&checkout).expect("remove"), Some("it has commits that aren't in the main checkout's branch".to_owned()));

        // main moved on: rebase first.
        commit(&repo, "other.rs", "theirs");
        let refused = land(&checkout).unwrap_err();
        assert!(refused.contains("Rebase onto it first (git rebase main)"), "{refused}");
        run(&tree, &["git", "-c", "user.name=T", "-c", "user.email=t@t", "rebase", "-q", "main"]);
        let landed = land(&checkout).expect("landed");
        assert!(landed.starts_with("Landed 1 commit on main"), "{landed}");
        assert_eq!(std::fs::read_to_string(repo.join("new.rs")).expect("landed file"), "work");
        assert!(land(&checkout).expect("again").starts_with("Nothing to land"));

        assert_eq!(remove(&checkout).expect("remove"), None);
        assert!(!tree.exists() && !checkout.exists());
        assert_eq!(run(&repo, &["git", "branch", "--list", &checkout.branch]), "", "its branch went with it");
    }
}
