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
//!
//! In the core, a marker line
//!
//! ```text
//! <!-- setting: <key> -->     or     <!-- setting: !<key> -->
//! ```
//!
//! makes the block after it (as above) only for agents whose context starts
//! while that setting is on (or, with `!`, off): `Manual::core_for`.

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
    pub notes: Vec<Note>,
    /// The core's lines, each with the settings it's only for (key, on).
    lines: Vec<(String, Vec<(String, bool)>)>,
}

impl Manual {
    /// The core as an agent reads it, `on` saying whether each setting a
    /// block is for is on.
    pub(crate) fn core_for(&self, on: impl Fn(&str) -> bool) -> String {
        let kept: Vec<&str> = self
            .lines
            .iter()
            .filter(|(_, when)| when.iter().all(|(key, wanted)| on(key) == *wanted))
            .map(|(line, _)| line.as_str())
            .collect();
        tidy(&kept)
    }
}

/// The address commits made by agents name Claktory as their co-author
/// with (`Co-authored-by: Claktory <…>`, the manual's `{{co_author}}`): None
/// until Claktory's own account on GitHub exists, and then no trailer is
/// asked for.
pub(crate) const CO_AUTHOR: Option<&str> = None;

static PARSED: LazyLock<Manual> = LazyLock::new(|| parse(MANUAL));

/// This build's manual.
pub(crate) fn manual() -> &'static Manual {
    &PARSED
}

/// `text` with `{{home}}`, `{{config}}` and `{{workspace}}` made this
/// Factory's paths, and `{{user}}` and `{{User}}` the user's name, or "the
/// person" and "The person" without one.
pub(crate) fn render(text: &str, home: &Home, user: Option<&str>) -> String {
    text.replace("{{user}}", user.unwrap_or("the person"))
        .replace("{{User}}", user.unwrap_or("The person"))
        .replace("{{config}}", &home.config_path().display().to_string())
        .replace("{{workspace}}", &home.workspace().display().to_string())
        .replace("{{home}}", &home.root().display().to_string())
        .replace("{{co_author}}", &CO_AUTHOR.map(|address| format!("Claktory <{address}>")).unwrap_or_default())
}

/// A setting marker line's key, and whether it's for the setting on.
fn setting(line: &str) -> Option<(&str, bool)> {
    let key = line.trim().strip_prefix("<!-- setting:")?.strip_suffix("-->")?.trim();
    Some(match key.strip_prefix('!') {
        Some(key) => (key.trim(), false),
        None => (key, true),
    })
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
        let marks = |line: &str| marker(line).is_some() || setting(line).is_some();
        while end < lines.len() && !ends(lines[end]) && !(marks(lines[end]) && lines.get(end + 1).is_some_and(|line| ends(line)))
        {
            end += 1;
        }
    } else {
        while end < lines.len()
            && !lines[end].trim().is_empty()
            && !lines[end].starts_with("- ")
            && heading(lines[end]).is_none()
            && marker(lines[end]).is_none()
            && setting(lines[end]).is_none()
        {
            end += 1;
        }
    }
    end
}

pub(crate) fn parse(text: &str) -> Manual {
    let lines: Vec<&str> = text.lines().collect();
    let mut core: Vec<(String, Vec<(String, bool)>)> = Vec::new();
    let mut notes = Vec::new();
    // The settings blocks are for, each until (exclusive) its line.
    let mut when: Vec<(String, bool, usize)> = Vec::new();
    let mut at = 0;
    while at < lines.len() {
        when.retain(|(_, _, end)| *end > at);
        if let Some((key, on)) = setting(lines[at]) {
            if at + 1 < lines.len() {
                when.push((key.to_owned(), on, block_end(&lines, at + 1)));
            }
            at += 1;
            continue;
        }
        let Some((slug, summary)) = marker(lines[at]) else {
            core.push((lines[at].to_owned(), when.iter().map(|(key, on, _)| (key.clone(), *on)).collect()));
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
    Manual { notes, lines: core }
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
        assert_eq!(manual.core_for(|_| true), "# Manual\n\nIntro.\n\n## Extensions\n\nHow they work.\n\n- **B**: does b.\n");
    }

    #[test]
    fn setting_blocks_are_only_for_agents_with_that_setting() {
        let text = "# Manual\n\nIntro.\n\n<!-- setting: credit -->\n## Credit\n\nSay so:\n- In a PR.\n<!-- setting: trailer -->\n- In a commit.\n\nOnly there.\n\n<!-- setting: !credit -->\n## Credit\n\nSay nothing.\n\n<!-- admin: a | A. -->\n## Admin\n\nA.\n\n## End\n\nBye.\n";
        let manual = parse(text);
        let core = |credit: bool, trailer: bool| manual.core_for(|key| if key == "credit" { credit } else { trailer });
        assert_eq!(
            core(true, true),
            "# Manual\n\nIntro.\n\n## Credit\n\nSay so:\n- In a PR.\n- In a commit.\n\nOnly there.\n\n## End\n\nBye.\n"
        );
        assert_eq!(core(true, false), "# Manual\n\nIntro.\n\n## Credit\n\nSay so:\n- In a PR.\n\nOnly there.\n\n## End\n\nBye.\n");
        assert_eq!(core(false, true), "# Manual\n\nIntro.\n\n## Credit\n\nSay nothing.\n\n## End\n\nBye.\n");
        assert_eq!(manual.notes.len(), 1);
        assert_eq!(manual.notes[0].body, "## Admin\n\nA.");
    }

    /// The real manual's credit for Claktory: the line on, no trailer
    /// until there's an address, and the other words off.
    #[test]
    fn the_manual_credits_claktory_unless_turned_off() {
        let co_author = CO_AUTHOR.is_some();
        let on = manual().core_for(|key| key == "attribution" || (key == "co_author" && co_author));
        assert!(on.contains("`Made with [Claktory](https://claktory.com)`"), "{on}");
        assert_eq!(on.contains("Co-authored-by:"), co_author);
        assert!(!on.contains("turned off crediting Claktory"));
        assert!(!on.contains("<!--"));
        let off = manual().core_for(|_| false);
        assert!(!off.contains("Made with [Claktory]"));
        assert!(off.contains("turned off crediting Claktory"), "{off}");
        assert!(on.contains("## Changing Claktory as an agent") && off.contains("## Changing Claktory as an agent"));
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
        let core = manual.core_for(|_| true);
        assert!(!core.contains("<!--"), "a marker that marks nothing");
        assert!(core.starts_with("# Claktory manual"));
        assert!(core.contains("## Agents"));
        assert!(core.len() < 16_000, "the core manual is {} bytes", core.len());
    }
}
