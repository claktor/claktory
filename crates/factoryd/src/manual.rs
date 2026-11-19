//! Factory's manual, docs/MANUAL.md, split in two: the core every agent
//! reads in its system prompt, and the admin notes only agents with the
//! Factory manual (the Memory extension's "factory" instance) open, one at a
//! time, as they need them.
//!
//! The manual stays one file for people to read and edit. A marker line
//!
//! ```text
//! <!-- admin: <slug> | <summary> -->
//! ```
//!
//! makes what follows it an admin note: a heading's whole section (up to the
//! next heading of the same or a higher level), or else one list item (up to
//! the next item, a blank line or a heading). Everything unmarked is core. A
//! heading left with nothing under it in the core goes too.

use std::sync::LazyLock;

use crate::home::Home;

const MANUAL: &str = include_str!("../../../docs/MANUAL.md");

pub(crate) struct Note {
    pub slug: String,
    /// One line.
    pub summary: String,
    /// The section or list item, as written.
    pub body: String,
}

pub(crate) struct Manual {
    pub core: String,
    pub notes: Vec<Note>,
}

static PARSED: LazyLock<Manual> = LazyLock::new(|| parse(MANUAL));

/// This build's manual.
pub(crate) fn manual() -> &'static Manual {
    &PARSED
}

/// `text` with `{{home}}`, `{{config}}` and `{{workspace}}` made this Factory's paths.
pub(crate) fn render(text: &str, home: &Home) -> String {
    text.replace("{{config}}", &home.config_path().display().to_string())
        .replace("{{workspace}}", &home.workspace().display().to_string())
        .replace("{{home}}", &home.root().display().to_string())
}

/// A marker line's slug and summary.
fn marker(line: &str) -> Option<(&str, &str)> {
    let inner = line.trim().strip_prefix("<!-- admin:")?.strip_suffix("-->")?;
    let (slug, summary) = inner.split_once('|')?;
    Some((slug.trim(), summary.trim()))
}

/// A heading's level: 2 for "## Agents".
fn heading(line: &str) -> Option<usize> {
    let level = line.chars().take_while(|c| *c == '#').count();
    (level > 0 && line[level..].starts_with(' ')).then_some(level)
}

/// Where the block starting at `lines[start]` ends (exclusive).
fn block_end(lines: &[&str], start: usize) -> usize {
    let mut end = start + 1;
    if let Some(level) = heading(lines[start]) {
        // A marker just before the next heading belongs to it.
        let ends = |line: &str| heading(line).is_some_and(|next| next <= level);
        while end < lines.len() && !ends(lines[end]) && !(marker(lines[end]).is_some() && lines.get(end + 1).is_some_and(|line| ends(line))) {
            end += 1;
        }
    } else {
        while end < lines.len()
            && !lines[end].trim().is_empty()
            && !lines[end].starts_with("- ")
            && heading(lines[end]).is_none()
            && marker(lines[end]).is_none()
        {
            end += 1;
        }
    }
    end
}

pub(crate) fn parse(text: &str) -> Manual {
    let lines: Vec<&str> = text.lines().collect();
    let mut core: Vec<&str> = Vec::new();
    let mut notes = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        let Some((slug, summary)) = marker(lines[at]) else {
            core.push(lines[at]);
            at += 1;
            continue;
        };
        let start = at + 1;
        if start >= lines.len() {
            break;
        }
        let end = block_end(&lines, start);
        let body = lines[start..end].join("\n").trim().to_owned();
        notes.push(Note { slug: slug.to_owned(), summary: summary.to_owned(), body });
        at = end;
    }
    Manual { core: tidy(&core), notes }
}

/// Drops headings left empty, and runs of blank lines.
fn tidy(lines: &[&str]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(level) = heading(line) {
            let rest = &lines[index + 1..];
            let next = rest.iter().position(|line| heading(line).is_some_and(|next| next <= level)).unwrap_or(rest.len());
            if rest[..next].iter().all(|line| line.trim().is_empty()) {
                continue;
            }
        }
        if line.trim().is_empty() && kept.last().is_none_or(|last| last.trim().is_empty()) {
            continue;
        }
        kept.push(line);
    }
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    let mut core = kept.join("\n");
    core.push('\n');
    core
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marked_sections_and_items_become_notes() {
        let text = "# Manual\n\nIntro.\n\n## App\n\n<!-- admin: app-sidebar | The sidebar. -->\n### Sidebar\n- One.\n- Two.\n\n<!-- admin: app-settings | Settings. -->\n### Settings\nText.\n\n## Extensions\n\nHow they work.\n\n<!-- admin: ext-a | A. -->\n- **A**: does a,\n  and more.\n<!-- admin: ext-c | C. -->\n- **C**: does c.\n- **B**: does b.\n\n## End\n";
        let manual = parse(text);
        let notes: Vec<(&str, &str, &str)> =
            manual.notes.iter().map(|note| (note.slug.as_str(), note.summary.as_str(), note.body.as_str())).collect();
        assert_eq!(
            notes,
            [
                ("app-sidebar", "The sidebar.", "### Sidebar\n- One.\n- Two."),
                ("app-settings", "Settings.", "### Settings\nText."),
                ("ext-a", "A.", "- **A**: does a,\n  and more."),
                ("ext-c", "C.", "- **C**: does c."),
            ]
        );
        assert_eq!(manual.core, "# Manual\n\nIntro.\n\n## Extensions\n\nHow they work.\n\n- **B**: does b.\n");
    }

    /// The real manual splits cleanly: notes with unique slugs and one-line
    /// summaries, and a core much smaller than the whole.
    #[test]
    fn the_manual_splits_into_a_core_and_admin_notes() {
        let manual = manual();
        assert!(manual.notes.len() >= 15, "{}", manual.notes.len());
        let mut slugs = std::collections::BTreeSet::new();
        for note in &manual.notes {
            assert!(note.slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{}", note.slug);
            assert!(slugs.insert(note.slug.as_str()), "{} twice", note.slug);
            assert!(!note.summary.is_empty() && note.summary.len() <= 150 && !note.summary.contains('\n'), "{}", note.slug);
            assert!(!note.body.is_empty(), "{} is empty", note.slug);
        }
        assert!(!manual.core.contains("<!--"), "a marker that marks nothing");
        assert!(manual.core.starts_with("# Factory manual"));
        assert!(manual.core.contains("## Agents"));
        assert!(manual.core.len() < 16_000, "the core manual is {} bytes", manual.core.len());
    }
}
