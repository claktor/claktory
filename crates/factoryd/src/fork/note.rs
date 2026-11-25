//! The note an agent gets when its conversation is taken back, or switched
//! to another version (`super`): what the part set aside did that still
//! stands, from Claktory's records since it began and from that part's own
//! tool calls. It quotes only the agent's own messages and the paths of
//! files it wrote, never what it read (an event, a tool's output, a pipe),
//! so it puts none of those words back in its context.

use std::collections::HashMap;
use std::fmt::Write as _;

use factory_agent::{AssistantPart, Message};

use super::{Branch, By};
use crate::factory::Factory;
use crate::transcript::local_time;

/// About the most a note holds.
const MOST: usize = 4096;
/// The most of each kind of thing it lists; past it, "and N more".
const EACH: usize = 10;
/// How much of a message it quotes.
const QUOTE: usize = 60;

/// What happened to the conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Change {
    /// Taken back to an earlier message.
    Back,
    /// Switched to another version.
    Switched,
}

/// "14:02" today, "23 Nov 14:02" another day, as History says it.
fn when(ms: i64) -> String {
    let day = |ms: i64| local_time(ms, "%Y-%m-%d");
    if day(ms) == day(crate::transcript::now_ms()) { local_time(ms, "%H:%M") } else { local_time(ms, "%-d %b %H:%M") }
}

/// The note's text, and its line for people: `change` is what the person
/// (or `by`) did, `told` how the conversation the agent remembers changed
/// (going back after a switch it hadn't heard of yet changes it to another
/// version).
#[allow(clippy::too_many_arguments)]
pub(super) fn write(
    factory: &Factory,
    id: &str,
    branch: &Branch,
    after: &[Message],
    closed: usize,
    by: By<'_>,
    change: Change,
    told: Change,
) -> (String, String) {
    let (person, who, subject, names) = {
        let inner = factory.lock();
        let config = &inner.config;
        let person = config.person().to_owned();
        // It starts the note and its line: "The person" when they have no name.
        let who = match config.agent(by.agent) {
            _ if by.agent.is_empty() => config.person_capitalized().to_owned(),
            Some(agent) => agent.name.clone(),
            None => by.agent.to_owned(),
        };
        let subject = config.agent(id).map_or_else(|| id.to_owned(), |agent| agent.name.clone());
        let names: HashMap<String, String> = config.agents.iter().map(|agent| (agent.id.clone(), agent.name.clone())).collect();
        (person, who, subject, names)
    };
    let name = |agent: &str| names.get(agent).cloned().unwrap_or_else(|| agent.to_owned());
    let since = branch.fork.ms;
    let (from, to) = (local_time(since, "%H:%M"), local_time(branch.set_aside_ms, "%H:%M"));
    let count = if branch.messages == 1 { "1 message".to_owned() } else { format!("{} messages", branch.messages) };
    let at = local_time(since, "%m-%d %H:%M");
    let mut text = match told {
        Change::Back => format!(
            "{who} took your conversation back to {at}. What came after ({from}–{to}, {count}) is kept as another version {person} can switch back to. You remember only what's above."
        ),
        Change::Switched => format!(
            "{who} switched you to another version of your conversation, which parts from the one you were in at {at}. The part you were in ({from}–{to}, {count}) is kept as another version {person} can switch back to. You remember only what's above."
        ),
    };
    let summary = match change {
        // The line people see names whose conversation it was: the person
        // reads it too, where "your" would be theirs (row 756).
        Change::Back => format!("{who} took {} conversation back to {}", crate::apps::changes::possessive(&subject), when(since)),
        Change::Switched => format!("{who} switched {subject} to another version"),
    };
    let reason = by.reason.trim();
    if !by.agent.is_empty() && !reason.is_empty() {
        let _ = write!(text, " {who}'s reason: {reason}");
    }
    text.push_str(" What that part did still stands:");
    for line in lines(factory, id, since, after, closed, &person, change, &name) {
        let _ = write!(text, "\n- {line}");
    }
    (capped(text), summary)
}

/// The note's lines: messages, files and commands always, the rest when
/// there's something to say.
#[allow(clippy::too_many_arguments)]
fn lines(
    factory: &Factory,
    id: &str,
    since: i64,
    after: &[Message],
    closed: usize,
    person: &str,
    change: Change,
    name: &dyn Fn(&str) -> String,
) -> Vec<String> {
    let db = factory.store.db();
    let mut lines = Vec::new();

    // Messages it sent: to agents, by whom, with the first one's words; and
    // through pipes, by pipe.
    let mut sent: Vec<(String, usize, String)> = Vec::new();
    let rows = db
        .prepare("SELECT agent, text FROM messages WHERE source = 'agent' AND sender_id = ?1 AND created >= ?2 ORDER BY id")
        .and_then(|mut statement| {
            statement.query_map(rusqlite::params![id, since], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?.collect()
        })
        .unwrap_or_else(|error: rusqlite::Error| {
            tracing::warn!(agent = %id, %error, "could not read the messages it sent");
            Vec::new()
        });
    for (to, words) in rows {
        match sent.iter_mut().find(|(agent, ..)| *agent == to) {
            Some((_, count, _)) => *count += 1,
            None => sent.push((to, 1, quote(&words))),
        }
    }
    let agents = listed(sent.iter().map(|(to, count, first)| format!("{} ×{count} (“{first}”)", name(to))).collect());
    let piped: Vec<(String, i64)> = db
        .prepare("SELECT pipe, COUNT(*) FROM pipe_messages WHERE agent = ?1 AND author = 'my_agent' AND created >= ?2 GROUP BY pipe ORDER BY MIN(seq)")
        .and_then(|mut statement| statement.query_map(rusqlite::params![id, since], crate::store::row2)?.collect())
        .unwrap_or_default();
    let pipes = listed(
        piped
            .iter()
            .map(|(pipe, count)| {
                let named = factory.pipes.get(pipe).map_or_else(|| pipe.clone(), |record| record.name.clone());
                format!("{named} ×{count}")
            })
            .collect(),
    );
    lines.push(format!("Messages you sent: {agents}. Pipe messages: {pipes}."));

    // Files it wrote, and how many commands it ran, from its own calls.
    let mut files: Vec<String> = Vec::new();
    let mut commands = 0;
    for message in after {
        let Message::Assistant(assistant) = message else { continue };
        for part in &assistant.content {
            let AssistantPart::ToolCall { name, arguments, .. } = part else { continue };
            match name.as_str() {
                "write_file" | "edit_file" => {
                    if let Some(path) = arguments.get("path").and_then(serde_json::Value::as_str)
                        && !files.iter().any(|file| file == path)
                    {
                        files.push(path.to_owned());
                    }
                }
                "bash" => commands += 1,
                _ => {}
            }
        }
    }
    let commands = match commands {
        0 => "none".to_owned(),
        count => format!("{count} (they may have changed files too)"),
    };
    lines.push(format!("Files you wrote or edited: {}. Commands run: {commands}.", listed(files)));

    // Its background tasks, and whether they still run.
    let tasks: Vec<String> = db
        .prepare("SELECT task, command, ended, status FROM tasks WHERE agent = ?1 AND started >= ?2 ORDER BY id")
        .and_then(|mut statement| {
            statement
                .query_map(rusqlite::params![id, since], |row| {
                    let (task, command): (String, String) = (row.get(0)?, row.get(1)?);
                    let (ended, status): (Option<i64>, Option<String>) = (row.get(2)?, row.get(3)?);
                    let state = match (ended, status) {
                        (None, _) => "still running".to_owned(),
                        (Some(_), Some(status)) if !status.is_empty() => format!("ended, {status}"),
                        (Some(_), _) => "ended".to_owned(),
                    };
                    Ok(format!("{task} `{}` ({state})", quote(&command)))
                })?
                .collect()
        })
        .unwrap_or_default();
    if !tasks.is_empty() {
        lines.push(format!("Background tasks: {}.", listed(tasks)));
    }

    // Monitors it set, its own or another agent's.
    let monitors: Vec<String> = db
        .prepare("SELECT agent, id, name, state FROM monitors WHERE created_by = ?1 AND created >= ?2 ORDER BY created")
        .and_then(|mut statement| {
            statement
                .query_map(rusqlite::params![id, since], |row| {
                    let (agent, number, named, state): (String, i64, String, String) = (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
                    let whose = if agent == id { String::new() } else { format!(" for {}", name(&agent)) };
                    Ok(format!("m{number} “{named}”{whose} ({state})"))
                })?
                .collect()
        })
        .unwrap_or_default();
    if !monitors.is_empty() {
        lines.push(format!("Monitors you added: {}.", listed(monitors)));
    }

    // What it did to checklists, its own or another agent's.
    let checklist: Vec<String> = db
        .prepare("SELECT agent, item, kind, value FROM checklist_events WHERE actor = ?1 AND time >= ?2 ORDER BY seq")
        .and_then(|mut statement| {
            statement
                .query_map(rusqlite::params![id, since], |row| {
                    let (agent, item, kind, value): (String, i64, String, String) = (row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?);
                    let item = if agent == id { format!("#{item}") } else { format!("{}'s #{item}", name(&agent)) };
                    Ok(match kind.as_str() {
                        "created" => format!("added {item}"),
                        "status" => format!("{item} marked {}", value.replace('_', " ")),
                        "owner" if value.is_empty() => format!("{item} unassigned"),
                        "owner" => format!("{item} given to {}", name(&value)),
                        "note" => format!("a note on {item}"),
                        _ => format!("{item} edited"),
                    })
                })?
                .collect()
        })
        .unwrap_or_default();
    if !checklist.is_empty() {
        lines.push(format!("Checklist: {}.", listed(checklist)));
    }

    if closed > 0 {
        let by = if change == Change::Back { "going back" } else { "the switch" };
        lines.push(format!("Questions to {person} closed by {by}: {closed}."));
    }
    lines
}

/// Up to `EACH` of `items`, with how many more; "none" for none.
fn listed(items: Vec<String>) -> String {
    if items.is_empty() {
        return "none".to_owned();
    }
    let more = items.len().saturating_sub(EACH);
    let mut said = items.into_iter().take(EACH).collect::<Vec<_>>().join(", ");
    if more > 0 {
        let _ = write!(said, " and {more} more");
    }
    said
}

/// The first words of `text`, on one line.
fn quote(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= QUOTE {
        return text;
    }
    let cut: String = text.chars().take(QUOTE).collect();
    format!("{}…", cut.trim_end())
}

/// `text`, cut to about `MOST` bytes at a line's end.
fn capped(text: String) -> String {
    if text.len() <= MOST {
        return text;
    }
    let mut end = MOST;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let cut = text[..end].rsplit_once('\n').map_or(&text[..end], |(kept, _)| kept);
    format!("{cut}\n- … and more, cut to keep this short.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_quotes_stay_short() {
        assert_eq!(listed(Vec::new()), "none");
        let many: Vec<String> = (1..=12).map(|number| format!("f{number}")).collect();
        assert_eq!(listed(many), "f1, f2, f3, f4, f5, f6, f7, f8, f9, f10 and 2 more");
        assert_eq!(quote("  one\n two "), "one two");
        assert!(quote(&"word ".repeat(30)).ends_with('…'));
        let long = format!("start{}", "\n- line".repeat(1000));
        let capped = capped(long);
        assert!(capped.len() < MOST + 100 && capped.ends_with("cut to keep this short."), "{}", capped.len());
    }
}
