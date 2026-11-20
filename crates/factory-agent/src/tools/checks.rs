//! Checks before a bash command starts, against two mistakes that block an
//! agent for minutes: waiting with a long `sleep`, and copying its own
//! repository (build output included). A guard against mistakes, not a
//! security boundary: `eval` or a script file gets around it.

use std::path::{Component, Path, PathBuf};

use crate::errors::AgentFailure;

/// The shortest leading `sleep` refused in the foreground, in seconds.
const LONGEST_SLEEP: f64 = 25.0;

/// The command's segments: split on `;`, `&`, `|` (so `&&` and `||` too) and
/// newlines, outside quotes.
fn segments(script: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        // `2>&1` and `&>` are redirections, not separators.
        if quote.is_none() && c == '&' && (current.ends_with(['>', '<']) || chars.peek() == Some(&'>')) {
            current.push(c);
            continue;
        }
        if escaped {
            escaped = false;
            current.push(c);
            continue;
        }
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('"'), '\\') | (None, '\\') => escaped = true,
            (None, '\'' | '"') => quote = Some(c),
            (None, ';' | '&' | '|' | '\n') => {
                segments.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    segments.push(current);
    segments.into_iter().map(|segment| segment.trim().to_owned()).filter(|segment| !segment.is_empty()).collect()
}

/// A command whose first segment is a `sleep`: stopped at its timeout
/// rather than moved to the background.
pub fn starts_with_sleep(script: &str) -> bool {
    segments(script).first().is_some_and(|first| first.split_whitespace().next() == Some("sleep"))
}

/// A foreground command that starts by sleeping 25 seconds or more.
pub fn long_sleep(script: &str) -> Option<AgentFailure> {
    let first = segments(script).into_iter().next()?;
    let mut words = first.split_whitespace();
    if words.next() != Some("sleep") {
        return None;
    }
    let seconds = words.next()?;
    if words.next().is_some() || !seconds.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    let value: f64 = seconds.parse().ok()?;
    (value >= LONGEST_SLEEP).then(|| AgentFailure::BashLongSleep { seconds: seconds.to_owned() })
}

/// The git top level `dir` is in, if any.
pub fn repository_root(dir: &Path) -> Option<PathBuf> {
    let dir = dir.canonicalize().ok()?;
    dir.ancestors().find(|dir| dir.join(".git").exists()).map(Path::to_path_buf)
}

/// A command that copies a whole repository in `roots`: `cp -R`/`-r`/`-a`,
/// `ditto`, `rsync`, `git clone` or `git worktree add`, from a folder that is
/// or contains one of them.
pub fn copies_repository(script: &str, cwd: &Path, roots: &[PathBuf]) -> Option<AgentFailure> {
    if roots.is_empty() {
        return None;
    }
    let mut cwd = cwd.to_path_buf();
    for segment in segments(script) {
        let Some(words) = shlex::split(&segment) else { continue };
        let words = without_redirections(words);
        let words = command_words(&words);
        let Some((program, rest)) = words.split_first() else { continue };
        let program = program.rsplit('/').next().unwrap_or(program);
        let sources: Vec<PathBuf> = match program {
            "cd" => {
                match rest.first().map(String::as_str) {
                    Some("-") => {}
                    Some(dir) => cwd = resolve(dir, &cwd),
                    None => cwd = resolve("~", &cwd),
                }
                continue;
            }
            "cp" if rest.iter().any(|word| recursive_cp(word)) => sources(operands(rest), &cwd),
            "ditto" | "rsync" => sources(operands(rest), &cwd),
            "git" => git_sources(rest, &cwd),
            _ => continue,
        };
        for source in sources {
            if let Some(root) = roots.iter().find(|root| root.starts_with(&source)) {
                return Some(AgentFailure::BashCopiesRepository { path: root.display().to_string() });
            }
        }
    }
    None
}

/// The words from the program on: leaves out variable assignments and
/// wrappers such as `sudo` and `time` in front.
fn command_words(words: &[String]) -> &[String] {
    let skip = words
        .iter()
        .take_while(|word| is_assignment(word) || matches!(word.as_str(), "sudo" | "time" | "nohup" | "exec" | "command" | "nice"))
        .count();
    &words[skip..]
}

/// The words without redirections: `> log`, `2>/dev/null`, `<input`.
fn without_redirections(words: Vec<String>) -> Vec<String> {
    let mut kept = Vec::with_capacity(words.len());
    let mut words = words.into_iter();
    while let Some(word) = words.next() {
        let operator = word.trim_start_matches(|c: char| c.is_ascii_digit() || c == '&');
        if operator.starts_with(['>', '<']) {
            if operator.trim_start_matches(['>', '<', '&', '|']).is_empty() {
                words.next();
            }
            continue;
        }
        kept.push(word);
    }
    kept
}

fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

fn recursive_cp(word: &str) -> bool {
    match word.strip_prefix("--") {
        Some(long) => long == "recursive" || long == "archive",
        None => word.strip_prefix('-').is_some_and(|flags| flags.contains(['R', 'r', 'a'])),
    }
}

/// The words that aren't options.
fn operands(words: &[String]) -> Vec<&str> {
    words.iter().map(String::as_str).filter(|word| !word.starts_with('-')).collect()
}

/// Every operand but the last (the destination); the only one when there's one.
fn sources(operands: Vec<&str>, cwd: &Path) -> Vec<PathBuf> {
    let count = if operands.len() > 1 { operands.len() - 1 } else { operands.len() };
    operands.into_iter().take(count).map(|operand| resolve(operand, cwd)).collect()
}

/// `git clone`'s source, or the repository `git worktree add` copies.
fn git_sources(words: &[String], cwd: &Path) -> Vec<PathBuf> {
    let mut base = cwd.to_path_buf();
    let mut index = 0;
    while index < words.len() {
        match words[index].as_str() {
            "-C" => {
                if let Some(dir) = words.get(index + 1) {
                    base = resolve(dir, &base);
                }
                index += 2;
            }
            "-c" => index += 2,
            word if word.starts_with('-') => index += 1,
            _ => break,
        }
    }
    let rest = words.get(index + 1..).unwrap_or_default();
    match words.get(index).map(String::as_str) {
        Some("clone") => {
            let operands: Vec<&str> =
                operands(rest).into_iter().map(|operand| operand.strip_prefix("file://").unwrap_or(operand)).collect();
            sources(operands, &base)
        }
        Some("worktree") if rest.first().map(String::as_str) == Some("add") => vec![repository_root(&base).unwrap_or(base)],
        _ => Vec::new(),
    }
}

/// `word` as an absolute path: `~` expanded, relative to `cwd`, `.` and `..`
/// resolved, a last part with a wildcard dropped (`dir/*` copies `dir`'s
/// contents), and symbolic links resolved when it exists.
fn resolve(word: &str, cwd: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = match (word.strip_prefix('~'), &home) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => cwd.join(word),
    };
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other),
        }
    }
    if normal.file_name().is_some_and(|name| name.to_string_lossy().contains(['*', '?', '['])) {
        normal.pop();
    }
    normal.canonicalize().unwrap_or(normal)
}
