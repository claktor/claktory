use std::cmp::Ordering;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::errors::{AgentError, AgentFailure, Error, FsError};
use crate::text::utf16_len;
use crate::tools::bounded;
use crate::types::ToolResult;

/// `path.resolve(cwd, input)`: absolute and lexically normalized, never touching the filesystem.
pub fn resolve(cwd: &Path, input: &str) -> PathBuf {
    let joined = if Path::new(input).is_absolute() { PathBuf::from(input) } else { cwd.join(input) };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn escape(input: &str) -> Error {
    AgentError::new(AgentFailure::WorkspaceEscape { path: input.to_owned() }).into()
}

/// Check addressed paths and existing symlink targets. The shell uses the enclosing environment.
pub async fn workspace_path(cwd: &Path, input: &str) -> Result<PathBuf, Error> {
    let absolute = resolve(cwd, input);
    if !absolute.starts_with(cwd) {
        return Err(escape(input));
    }
    let root = tokio::fs::canonicalize(cwd).await.map_err(|error| Error::fs(error, "realpath", cwd))?;
    let mut existing = absolute.clone();
    loop {
        match tokio::fs::canonicalize(&existing).await {
            Ok(real) => return if real.starts_with(&root) { Ok(absolute) } else { Err(escape(input)) },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && existing != cwd => match existing.parent() {
                Some(parent) => existing = parent.to_path_buf(),
                None => return Err(Error::fs(error, "realpath", &existing)),
            },
            Err(error) => return Err(Error::fs(error, "realpath", &existing)),
        }
    }
}

/// UTF-8 with replacement characters, as `readFile(path, "utf8")`; a directory fails on `read` like Node.
pub async fn read_text(target: &Path) -> Result<String, Error> {
    match tokio::fs::read(target).await {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
        Err(error) if error.raw_os_error() == Some(libc::EISDIR) => Err(FsError::new(error, "read", None, None).into()),
        Err(error) => Err(Error::fs(error, "open", target)),
    }
}

pub async fn replace_file(target: &Path, content: &str) -> Result<(), Error> {
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|error| Error::fs(error, "mkdir", parent))?;
    }
    let temp = PathBuf::from(format!("{}.tmp", target.display()));
    let written = match tokio::fs::write(&temp, content).await {
        Ok(()) => tokio::fs::rename(&temp, target).await.map_err(|error| Error::fs_rename(error, &temp, target)),
        Err(error) => Err(Error::fs(error, "open", &temp)),
    };
    if written.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    written
}

fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' => {
                lines.push(&text[start..i]);
                i += if bytes.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                start = i;
            }
            b'\n' => {
                lines.push(&text[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    lines.push(&text[start..]);
    lines
}

fn aborted(cancel: &CancellationToken) -> Result<(), Error> {
    if cancel.is_cancelled() { Err(Error::Aborted) } else { Ok(()) }
}

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
    offset: Option<u64>,
    limit: Option<u64>,
}

pub async fn read_file(cwd: &Path, args: &Value, cancel: &CancellationToken) -> Result<ToolResult, Error> {
    let args: ReadArgs = serde_json::from_value(args.clone())?;
    let offset = args.offset.unwrap_or(1) as usize;
    let limit = args.limit.unwrap_or(200) as usize;
    let target = workspace_path(cwd, &args.path).await?;
    aborted(cancel)?;
    let text = read_text(&target).await?;
    let lines = split_lines(&text);
    let end = lines.len().min(offset - 1 + limit);
    let shown: Vec<String> =
        lines.get(offset - 1..end).unwrap_or(&[]).iter().enumerate().map(|(i, line)| format!("{}: {line}", offset + i)).collect();
    let text = format!("{}\n[Lines {offset}-{end} of {}]", bounded(&shown.join("\n")), lines.len());
    Ok(ToolResult::text(text, json!({"path": args.path, "offset": offset, "end": end, "totalLines": lines.len()})))
}

#[derive(Deserialize)]
struct WriteArgs {
    path: String,
    content: String,
}

pub async fn write_file(cwd: &Path, args: &Value, cancel: &CancellationToken) -> Result<ToolResult, Error> {
    let args: WriteArgs = serde_json::from_value(args.clone())?;
    let target = workspace_path(cwd, &args.path).await?;
    aborted(cancel)?;
    let existed = tokio::fs::metadata(&target).await.is_ok();
    replace_file(&target, &args.content).await?;
    let text = format!("{} {} ({} characters).", if existed { "Replaced" } else { "Created" }, args.path, utf16_len(&args.content));
    Ok(ToolResult::text(text, json!({"path": args.path, "existed": existed})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Edit {
    old_text: String,
    new_text: String,
    replace_all: Option<bool>,
}

#[derive(Deserialize)]
struct EditArgs {
    path: String,
    edits: Vec<Edit>,
}

fn invalid_edit(path: &str, reason: &str) -> Error {
    AgentError::new(AgentFailure::InvalidEdit { path: path.to_owned(), reason: reason.to_owned() }).into()
}

type Line<'a> = (usize, &'a str);

fn lines_at(text: &str) -> Vec<Line<'_>> {
    let mut start = 0;
    text.split('\n')
        .map(|line| {
            let at = start;
            start += line.len() + 1;
            (at, line)
        })
        .collect()
}

fn occurrences(text: &str, needle: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find(needle) {
        let start = from + found;
        starts.push(start);
        from = start + text[start..].chars().next().map_or(1, char::len_utf8);
    }
    starts
}

/// Whole-line windows equal to `old` once each line passes through `norm`; a trailing newline in `old` stays in the span.
fn line_windows(lines: &[Line], old: &str, norm: fn(&str) -> &str) -> Vec<Range<usize>> {
    let body = old.strip_suffix('\n');
    let wanted: Vec<&str> = body.unwrap_or(old).split('\n').map(norm).collect();
    let newline = usize::from(body.is_some());
    lines
        .windows(wanted.len() + newline)
        .filter(|window| window.iter().zip(&wanted).all(|((_, line), want)| norm(line) == *want))
        .map(|window| {
            let (last_start, last) = window[wanted.len() - 1];
            window[0].0..last_start + last.len() + newline
        })
        .collect()
}

fn bigrams(text: &str) -> Vec<(char, char)> {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut pairs: Vec<(char, char)> = chars.windows(2).map(|pair| (pair[0], pair[1])).collect();
    pairs.sort_unstable();
    pairs
}

fn dice(a: &[(char, char)], b: &[(char, char)]) -> f64 {
    let (mut i, mut j, mut shared) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            Ordering::Less => i += 1,
            Ordering::Greater => j += 1,
            Ordering::Equal => (i, j, shared) = (i + 1, j + 1, shared + 1),
        }
    }
    if a.is_empty() || b.is_empty() { 0.0 } else { 2.0 * shared as f64 / (a.len() + b.len()) as f64 }
}

fn not_found(lines: &[Line], old: &str) -> String {
    let mut reason = String::from("oldText not found");
    let Some(first) = old.split('\n').find(|line| !line.trim().is_empty()) else { return reason };
    let wanted = bigrams(first);
    let mut close: Vec<(f64, usize, &str)> = lines
        .iter()
        .enumerate()
        .map(|(index, (_, line))| (dice(&wanted, &bigrams(line)), index + 1, *line))
        .filter(|(score, ..)| *score >= 0.5)
        .collect();
    close.sort_by(|a, b| b.0.total_cmp(&a.0));
    let shown: Vec<String> = close
        .into_iter()
        .take(3)
        .map(|(_, number, line)| {
            let clipped: String = line.chars().take(120).collect();
            format!("\n  {number}: {clipped}{}", if clipped.len() < line.len() { "…" } else { "" })
        })
        .collect();
    if !shown.is_empty() {
        reason.push_str("; closest lines:");
        reason.push_str(&shown.concat());
    }
    reason
}

fn ambiguous(lines: &[Line], ranges: &[Range<usize>], qualifier: &str, advice: &str) -> String {
    let mut numbers: Vec<usize> = ranges.iter().map(|range| lines.partition_point(|(at, _)| *at <= range.start)).collect();
    numbers.dedup();
    let mut listed: Vec<String> = numbers.iter().take(10).map(usize::to_string).collect();
    if numbers.len() > 10 {
        listed.push("…".into());
    }
    format!("oldText matches {} times{qualifier}, at lines {}; {advice}", ranges.len(), listed.join(", "))
}

/// The ranges `old` replaces, and whether they matched only after ignoring whitespace.
fn locate(original: &str, lines: &[Line], old: &str, replace_all: bool) -> Result<(Vec<Range<usize>>, bool), String> {
    if old.is_empty() {
        return Err("oldText is empty".into());
    }
    if replace_all {
        let ranges: Vec<Range<usize>> = original.match_indices(old).map(|(start, _)| start..start + old.len()).collect();
        return if ranges.is_empty() { Err(not_found(lines, old)) } else { Ok((ranges, false)) };
    }
    let ranges: Vec<Range<usize>> = occurrences(original, old).into_iter().map(|start| start..start + old.len()).collect();
    match ranges.len() {
        0 => {}
        1 => return Ok((ranges, false)),
        _ => return Err(ambiguous(lines, &ranges, "", "add surrounding context or set replaceAll")),
    }
    if old.trim().is_empty() {
        return Err(not_found(lines, old));
    }
    let mut spans = line_windows(lines, old, str::trim_end);
    if spans.len() != 1 {
        spans = line_windows(lines, old, str::trim);
    }
    match spans.len() {
        0 => Err(not_found(lines, old)),
        1 => Ok((spans, true)),
        _ => Err(ambiguous(lines, &spans, " after ignoring whitespace", "add surrounding context")),
    }
}

pub async fn edit_file(cwd: &Path, args: &Value, cancel: &CancellationToken) -> Result<ToolResult, Error> {
    let args: EditArgs = serde_json::from_value(args.clone())?;
    let target = workspace_path(cwd, &args.path).await?;
    aborted(cancel)?;
    let before = read_text(&target).await?;
    let newline = if before.contains("\r\n") { "\r\n" } else { "\n" };
    let original = before.replace("\r\n", "\n");
    let lines = lines_at(&original);
    let mut failures = Vec::new();
    let mut loose = Vec::new();
    let mut replacements = Vec::new();
    for (index, edit) in args.edits.iter().enumerate() {
        match locate(&original, &lines, &edit.old_text.replace("\r\n", "\n"), edit.replace_all == Some(true)) {
            Ok((ranges, whitespace)) => {
                if whitespace {
                    loose.push(format!("edits[{index}]"));
                }
                let text = edit.new_text.replace("\r\n", "\n");
                replacements.extend(ranges.into_iter().map(|range| (range, index, text.clone())));
            }
            Err(reason) => failures.push(format!("edits[{index}]: {reason}")),
        }
    }
    replacements.sort_by_key(|(range, _, _)| range.start);
    let mut overlaps: Vec<(usize, usize)> = replacements
        .windows(2)
        .filter(|pair| pair[1].0.start < pair[0].0.end)
        .map(|pair| (pair[0].1.min(pair[1].1), pair[0].1.max(pair[1].1)))
        .collect();
    overlaps.sort_unstable();
    overlaps.dedup();
    failures.extend(overlaps.into_iter().map(|(a, b)| format!("edits[{a}] and edits[{b}]: replacement ranges overlap")));
    if !failures.is_empty() {
        return Err(invalid_edit(&args.path, &failures.join("\n")));
    }
    let mut after = original.clone();
    for (range, _, text) in replacements.iter().rev() {
        after.replace_range(range.clone(), text);
    }
    let after = after.replace('\n', newline);
    aborted(cancel)?;
    if read_text(&target).await? != before {
        return Err(invalid_edit(&args.path, "file changed while preparing the edit"));
    }
    tokio::fs::write(&target, &after).await.map_err(|error| Error::fs(error, "open", &target))?;
    let diff = crate::tools::diff::two_files_patch(&args.path, &args.path, &before, &after);
    let mut text = bounded(&diff);
    if !loose.is_empty() {
        text = format!("{} matched only after ignoring whitespace.\n{text}", loose.join(", "));
    }
    Ok(ToolResult::text(text, json!({"path": args.path, "diff": diff})))
}
