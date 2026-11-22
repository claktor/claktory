//! Find in a conversation (SearchConversation): an agent's or a chat's,
//! since its last /clear and not only what the app holds, or a pipe's
//! timeline. The transcript is in memory once loaded, so it's scanned
//! newest first in short slices, the lock let go between them, so a
//! running agent is never held up for long.

use std::ops::Range;

use factory_protocol::v1::{self, Entry, EventSource, entry::Kind};
use regex::{Regex, RegexBuilder};
use rusqlite::params;

use crate::factory::{Factory, RequestError};

/// Matching entries per page: when the request says 0, and at most.
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 500;
/// Occurrences counted for the first page's total, at most.
const TOTAL_CAP: u32 = 1000;
/// The longest query, in characters.
const MAX_QUERY: usize = 500;
/// Entries scanned per hold of the lock.
const SLICE: usize = 2000;
/// A snippet's length, and where its first occurrence starts, in characters.
const SNIPPET: usize = 80;
const LEAD: usize = 25;

/// SearchConversation.
pub(crate) async fn search(factory: &Factory, request: &v1::SearchConversation) -> Result<v1::ConversationMatches, RequestError> {
    let targets = [&request.agent_id, &request.chat_id, &request.pipe_id].iter().filter(|id| !id.is_empty()).count();
    if targets != 1 {
        return Err(RequestError::Find("Say which conversation to search: an agent, a chat or a pipe.".to_owned()));
    }
    if request.query.chars().count() > MAX_QUERY {
        return Err(RequestError::TooLong { what: "A search", most: MAX_QUERY });
    }
    let limit = match request.limit as usize {
        0 => DEFAULT_LIMIT,
        limit => limit.min(MAX_LIMIT),
    };
    let mut answer = v1::ConversationMatches {
        query: request.query.clone(),
        agent_id: request.agent_id.clone(),
        chat_id: request.chat_id.clone(),
        pipe_id: request.pipe_id.clone(),
        before_seq: request.before_seq,
        ..Default::default()
    };
    if !request.pipe_id.is_empty() {
        if factory.pipes.get(&request.pipe_id).is_none() {
            return Err(RequestError::NoPipe(request.pipe_id.clone()));
        }
    } else if !request.chat_id.is_empty() {
        if !factory.chats.exists(&request.chat_id) {
            return Err(RequestError::Account(v1::ErrorCode::NotFound, format!("No chat \"{}\".", request.chat_id)));
        }
    } else {
        factory.known_agent(&request.agent_id)?;
    }
    let Some(finder) = Finder::new(&request.query) else { return Ok(answer) };
    let mut found = Found { limit, counting: request.before_seq.is_none(), ..Found::default() };
    if !request.pipe_id.is_empty() {
        search_pipe(factory, &request.pipe_id, request.before_seq, &finder, &mut found)?;
    } else {
        let id = if request.chat_id.is_empty() { &request.agent_id } else { &request.chat_id };
        factory.ensure_loaded(id).await?;
        search_transcript(factory, id, request.before_seq, &finder, &mut found).await;
    }
    answer.more = found.more;
    answer.total = found.total.min(TOTAL_CAP);
    answer.total_capped = found.total > TOTAL_CAP;
    answer.matches = found.matches;
    Ok(answer)
}

/// What a search has found so far.
#[derive(Default)]
struct Found {
    limit: usize,
    /// The first page counts every occurrence, up to the cap.
    counting: bool,
    matches: Vec<v1::ConversationMatch>,
    more: bool,
    total: u32,
}

impl Found {
    /// Adds an entry's match; whether the search can stop.
    fn add(&mut self, hit: v1::ConversationMatch) -> bool {
        if self.counting {
            self.total = self.total.saturating_add(hit.hits);
        }
        if self.matches.len() < self.limit {
            self.matches.push(hit);
        } else {
            self.more = true;
        }
        self.more && (!self.counting || self.total > TOTAL_CAP)
    }
}

async fn search_transcript(factory: &Factory, id: &str, before: Option<u64>, finder: &Finder, found: &mut Found) {
    let mut end = before.map_or(usize::MAX, |before| usize::try_from(before).unwrap_or(usize::MAX));
    loop {
        let start = {
            let inner = factory.lock();
            let Some(transcript) = inner.agents.get(id).and_then(|runtime| runtime.transcript.as_ref()) else { return };
            let entries = transcript.entries();
            end = end.min(entries.len());
            let start = end.saturating_sub(SLICE);
            for entry in entries[start..end].iter().rev() {
                if let Some(hit) = finder.entry(entry)
                    && found.add(hit)
                {
                    return;
                }
            }
            start
        };
        if start == 0 {
            return;
        }
        end = start;
        tokio::task::yield_now().await;
    }
}

fn search_pipe(factory: &Factory, pipe: &str, before: Option<u64>, finder: &Finder, found: &mut Found) -> Result<(), RequestError> {
    let before = before.map_or(i64::MAX, |before| i64::try_from(before).unwrap_or(i64::MAX));
    let db = factory.store.db();
    let mut statement = db.prepare("SELECT seq, created, text, note FROM pipe_messages WHERE pipe = ?1 AND seq < ?2 ORDER BY seq DESC")?;
    let mut rows = statement.query(params![pipe, before])?;
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let created: i64 = row.get(1)?;
        let text: String = row.get(2)?;
        let note: String = row.get(3)?;
        let texts = [text.as_str(), note.as_str()];
        if let Some(hit) = finder.texts(u64::try_from(seq).unwrap_or_default(), created, &texts)
            && found.add(hit)
        {
            break;
        }
    }
    Ok(())
}

/// The query, ready to look for.
struct Finder {
    plain: Regex,
    /// The query as it reads inside a JSON string (a tool call's input),
    /// to pass over calls it can't be in without parsing them.
    json: Regex,
}

impl Finder {
    /// None for a blank query, which finds nothing.
    fn new(query: &str) -> Option<Finder> {
        if query.trim().is_empty() {
            return None;
        }
        let build = |text: &str| RegexBuilder::new(&regex::escape(text)).case_insensitive(true).size_limit(1 << 24).build().ok();
        let escaped = serde_json::to_string(query).ok()?;
        let escaped = &escaped[1..escaped.len() - 1];
        Some(Finder { plain: build(query)?, json: build(escaped)? })
    }

    fn entry(&self, entry: &Entry) -> Option<v1::ConversationMatch> {
        let shown = |text: &str| self.texts(entry.seq, entry.timestamp_ms, &[text]);
        match entry.kind.as_ref()? {
            Kind::User(user) => shown(&user.text),
            Kind::Assistant(assistant) => shown(&assistant.text),
            Kind::Event(event) => {
                if event.source() == EventSource::Agent || event.summary.is_empty() {
                    shown(&event.text)
                } else {
                    shown(&event.summary)
                }
            }
            Kind::ToolCall(call) => {
                // The input's strings, then the output as the app shows it
                // when the call is open. The input is parsed only when the
                // query can be in it.
                let parsed = self.json.is_match(&call.input).then(|| serde_json::from_str::<serde_json::Value>(&call.input));
                let mut texts = Vec::new();
                match &parsed {
                    Some(Ok(input)) => string_values(input, &mut texts),
                    // Still being written: as it is so far.
                    Some(Err(_)) => texts.push(call.input.as_str()),
                    None => {}
                }
                texts.push(&call.output);
                self.texts(entry.seq, entry.timestamp_ms, &texts)
            }
            _ => None,
        }
    }

    /// A match in the first of `texts` the query occurs in, counting the
    /// occurrences in all of them.
    fn texts(&self, seq: u64, timestamp_ms: i64, texts: &[&str]) -> Option<v1::ConversationMatch> {
        let mut found: Option<v1::ConversationMatch> = None;
        let mut hits = 0u32;
        for text in texts {
            let ranges: Vec<Range<usize>> = self.plain.find_iter(text).map(|hit| hit.range()).collect();
            if ranges.is_empty() {
                continue;
            }
            hits = hits.saturating_add(u32::try_from(ranges.len()).unwrap_or(u32::MAX));
            if found.is_none() {
                let (snippet, starts, lengths) = snippet(text, &ranges);
                found = Some(v1::ConversationMatch { seq, snippet, starts, lengths, hits: 0, timestamp_ms });
            }
        }
        let mut found = found?;
        found.hits = hits;
        Some(found)
    }
}

/// Every string in a JSON value, in order: a tool call's arguments as the
/// app shows them, without keys or punctuation.
fn string_values<'a>(value: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(text) => out.push(text),
        serde_json::Value::Array(items) => items.iter().for_each(|item| string_values(item, out)),
        serde_json::Value::Object(map) => map.values().for_each(|item| string_values(item, out)),
        _ => {}
    }
}

/// About `SNIPPET` characters of `text` on one line around the first of
/// `hits` (byte ranges, in order), and where each hit inside it starts and
/// how long it is, in UTF-16 code units.
fn snippet(text: &str, hits: &[Range<usize>]) -> (String, Vec<u32>, Vec<u32>) {
    let first = hits[0].start;
    // LEAD characters before the first hit, then up to SNIPPET in all.
    let start = text[..first].char_indices().rev().nth(LEAD - 1).map_or(0, |(at, _)| at);
    let end = text[start..].char_indices().nth(SNIPPET).map_or(text.len(), |(at, _)| start + at);
    let mut out = String::new();
    let mut units = 0u32;
    if start > 0 {
        out.push('…');
        units += 1;
    }
    // The UTF-16 position in `out` of each char boundary of the window.
    let mut at: Vec<(usize, u32)> = Vec::new();
    let mut space = start == 0;
    for (offset, c) in text[start..end].char_indices() {
        at.push((start + offset, units));
        if c.is_whitespace() {
            if !space {
                out.push(' ');
                units += 1;
                space = true;
            }
        } else {
            out.push(c);
            units += c.len_utf16() as u32;
            space = false;
        }
    }
    at.push((end, units));
    let position = |byte: usize| match at.binary_search_by_key(&byte, |(byte, _)| *byte) {
        Ok(index) => at[index].1,
        Err(index) => at[index.min(at.len() - 1)].1,
    };
    let mut starts = Vec::new();
    let mut lengths = Vec::new();
    for hit in hits.iter().filter(|hit| hit.start >= start && hit.start < end) {
        let from = position(hit.start);
        let to = position(hit.end.min(end));
        if to > from {
            starts.push(from);
            lengths.push(to - from);
        }
    }
    if end < text.len() {
        if out.ends_with(' ') {
            out.pop();
        }
        out.push('…');
    }
    (out, starts, lengths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(text: &str, start: u32, length: u32) -> String {
        let units: Vec<u16> = text.encode_utf16().collect();
        String::from_utf16(&units[start as usize..(start + length) as usize]).unwrap()
    }

    #[test]
    fn a_snippet_is_one_line_around_the_first_hit_in_utf16() {
        let finder = Finder::new("ÉTÉ").unwrap();
        let text = format!("{}\n\n  the été 🌞 and Été\tagain {}", "x".repeat(100), "y".repeat(100));
        let hit = finder.texts(7, 9, &[&text]).unwrap();
        assert_eq!(hit.hits, 2);
        assert!(hit.snippet.starts_with('…') && hit.snippet.ends_with('…'), "{}", hit.snippet);
        assert!(!hit.snippet.contains('\n') && !hit.snippet.contains("  "), "{}", hit.snippet);
        assert_eq!(hit.starts.len(), 2);
        assert_eq!(utf16(&hit.snippet, hit.starts[0], hit.lengths[0]), "été");
        assert_eq!(utf16(&hit.snippet, hit.starts[1], hit.lengths[1]), "Été");
        assert!(hit.snippet[..hit.snippet.find("été").unwrap()].chars().count() <= LEAD + 1);
    }

    #[test]
    fn a_short_text_is_whole_and_blank_finds_nothing() {
        let hit = Finder::new("cat").unwrap().texts(0, 0, &["A Cat, a cat."]).unwrap();
        assert_eq!((hit.snippet.as_str(), hit.starts, hit.lengths, hit.hits), ("A Cat, a cat.", vec![2, 9], vec![3, 3], 2));
        assert!(Finder::new("  ").is_none());
        assert!(Finder::new("dog").unwrap().texts(0, 0, &["A Cat"]).is_none());
    }

    #[test]
    fn tool_calls_match_their_values_not_their_keys() {
        let call = |input: &str| Entry {
            seq: 3,
            kind: Some(Kind::ToolCall(v1::ToolCall { name: "bash".into(), input: input.into(), ..Default::default() })),
            ..Default::default()
        };
        let input = "{\n  \"command\": \"grep \\\"needle\\\" src\"\n}";
        assert!(Finder::new("command").unwrap().entry(&call(input)).is_none());
        assert!(Finder::new("bash").unwrap().entry(&call(input)).is_none());
        let mut ran = call(input);
        if let Some(Kind::ToolCall(call)) = &mut ran.kind {
            call.output = "src/a.rs: needle\nsrc/b.rs: no NEEDLE here".into();
        }
        let hit = Finder::new("needle").unwrap().entry(&ran).unwrap();
        assert_eq!((hit.snippet.as_str(), hit.hits), ("grep \"needle\" src", 3));
        let hit = Finder::new("b.rs").unwrap().entry(&ran).unwrap();
        assert_eq!((hit.snippet.as_str(), hit.starts.as_slice()), ("src/a.rs: needle src/b.rs: no NEEDLE here", &[21][..]));
        let hit = Finder::new("\"Needle\"").unwrap().entry(&call(input)).unwrap();
        assert_eq!(hit.snippet, "grep \"needle\" src");
        assert_eq!((hit.starts, hit.lengths), (vec![5], vec![8]));
    }
}
