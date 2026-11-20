//! Where a comment's text is, and where it went after the file changed.
//!
//! Text is handled as UTF-16 code units, the offsets the protocol and the
//! app use. A thread's place is a range of the file's text; its quote (the
//! text and some context on each side) is in its own space: the file's text,
//! or for a Markdown file's formatted view, the plain text it shows
//! (`Projection`), with runs of whitespace as one space.
//!
//! When the file changes, a range is mapped through the diff between the
//! text it was on and the new text, and checked against its quote; if that
//! fails, the quote is searched for, approximately, scored on its context
//! and nearness the way Hypothesis does (quote 50, prefix 20, suffix 20,
//! position 2).

use std::collections::HashMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use pulldown_cmark::{Event, Options, Parser, TagEnd};
use similar::{Algorithm, DiffTag, capture_diff_slices_deadline};

/// Units of context kept on each side of a quote.
pub(crate) const CONTEXT: usize = 32;

/// How long a diff may take before it settles for a coarser one.
const DIFF_TIME: Duration = Duration::from_millis(250);

pub(crate) fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

pub(crate) fn string(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

/// `text` with every run of whitespace as one space.
pub(crate) fn collapse(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut space = false;
    for character in text.chars() {
        if character.is_whitespace() {
            space = true;
            continue;
        }
        if space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        space = false;
        collapsed.push(character);
    }
    if space && !collapsed.is_empty() {
        collapsed.push(' ');
    }
    collapsed
}

fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..0xE000).contains(&unit)
}

/// `at`, moved back off the middle of a character.
fn boundary(text: &[u16], at: usize) -> usize {
    let at = at.min(text.len());
    if at > 0 && at < text.len() && is_low_surrogate(text[at]) { at - 1 } else { at }
}

/// What a thread quotes: its text, and some context on each side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Quote {
    pub exact: Vec<u16>,
    pub prefix: Vec<u16>,
    pub suffix: Vec<u16>,
}

impl Quote {
    /// The quote of `range` in `text`.
    pub(crate) fn of(text: &[u16], range: &Range<usize>) -> Quote {
        let (start, end) = (boundary(text, range.start), boundary(text, range.end.max(range.start)));
        let before = boundary(text, start.saturating_sub(CONTEXT));
        let after = boundary(text, (end + CONTEXT).min(text.len()));
        Quote { exact: text[start..end].to_vec(), prefix: text[before..start].to_vec(), suffix: text[end..after].to_vec() }
    }

    /// From strings, as clients and agents give them.
    pub(crate) fn new(exact: &str, prefix: &str, suffix: &str) -> Quote {
        Quote { exact: units(exact), prefix: units(prefix), suffix: units(suffix) }
    }

    /// With whitespace collapsed, for a Markdown file's formatted view: the
    /// quote trimmed, its context keeping the space next to it.
    pub(crate) fn collapsed(&self) -> Quote {
        let exact = collapse(&string(&self.exact));
        let prefix = collapse(&string(&self.prefix));
        let suffix = collapse(&string(&self.suffix));
        let (lead, trail) = (exact.starts_with(' '), exact.ends_with(' '));
        let prefix = if lead && !prefix.ends_with(' ') { format!("{prefix} ") } else { prefix };
        let suffix = if trail && !suffix.starts_with(' ') { format!(" {suffix}") } else { suffix };
        Quote { exact: units(exact.trim()), prefix: units(prefix.trim_start()), suffix: units(suffix.trim_end()) }
    }
}

/// How alike two texts are, from 0 to 1.
pub(crate) fn similarity(a: &[u16], b: &[u16]) -> f32 {
    if a == b {
        return 1.0;
    }
    let ops = capture_diff_slices_deadline(Algorithm::Myers, a, b, Some(Instant::now() + DIFF_TIME));
    similar::diff_ratio(&ops, a.len(), b.len())
}

/// Where at most `limit` copies of `needle` start in `text`.
fn occurrences(text: &[u16], needle: &[u16], limit: usize) -> Vec<usize> {
    let Some(&first) = needle.first() else { return Vec::new() };
    let mut found = Vec::new();
    if needle.len() > text.len() {
        return found;
    }
    for at in 0..=text.len() - needle.len() {
        if text[at] == first && text[at..at + needle.len()] == *needle {
            found.push(at);
            if found.len() == limit {
                break;
            }
        }
    }
    found
}

/// A quote found in a text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Found {
    pub range: Range<usize>,
    /// How alike the text there is to the quote: 1 when it's the same.
    pub similarity: f32,
}

/// How well `range` of `text` fits `quote` (whose text there is `alike` to
/// it): its score, and how alike its context is (None without context).
fn score(text: &[u16], quote: &Quote, range: &Range<usize>, alike: f32, hint: Option<usize>) -> (f32, Option<f32>) {
    let (mut total, mut weight) = (50.0 * alike, 50.0);
    let mut context = Vec::new();
    if !quote.prefix.is_empty() {
        let before = &text[range.start.saturating_sub(quote.prefix.len())..range.start];
        context.push(similarity(&quote.prefix, before));
    }
    if !quote.suffix.is_empty() {
        let after = &text[range.end..(range.end + quote.suffix.len()).min(text.len())];
        context.push(similarity(&quote.suffix, after));
    }
    for alike in &context {
        total += 20.0 * alike;
        weight += 20.0;
    }
    if let Some(hint) = hint {
        let distance = range.start.abs_diff(hint) as f32 / text.len().max(1) as f32;
        total += 2.0 * (1.0 - distance.min(1.0));
        weight += 2.0;
    }
    let context = (!context.is_empty()).then(|| context.iter().sum::<f32>() / context.len() as f32);
    (total / weight, context)
}

/// Finds `quote` in `text`: the copy that fits its context and `hint` (where
/// it's expected to start) best, or failing that, the best text like it.
pub(crate) fn find(text: &[u16], quote: &Quote, hint: Option<usize>) -> Option<Found> {
    if quote.exact.is_empty() {
        return None;
    }
    let exact = occurrences(text, &quote.exact, 1000);
    let best = exact
        .into_iter()
        .map(|start| {
            let range = start..start + quote.exact.len();
            let (score, _) = score(text, quote, &range, 1.0, hint);
            (score, range)
        })
        .max_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, range)) = best {
        return Some(Found { range, similarity: 1.0 });
    }
    approximate(text, quote, hint)
}

/// The best match of `pattern` in `text` (semi-global edit distance): its
/// range and distance. Text the file has that the pattern doesn't costs a
/// quarter of what the pattern's own text missing costs, since rewriting
/// adds words to a passage more often than it cuts its end.
fn align_in(text: &[u16], pattern: &[u16]) -> Option<(Range<usize>, usize)> {
    const MISSING: usize = 4;
    const EXTRA: usize = 1;
    let rows = pattern.len();
    if rows == 0 || text.is_empty() {
        return None;
    }
    // cost[i]: the distance of pattern[..i] to the best text ending here;
    // start[i]: where that text starts.
    let mut cost: Vec<usize> = (0..=rows).map(|row| row * MISSING).collect();
    let mut start = vec![0; rows + 1];
    let mut best: Option<(Range<usize>, usize)> = None;
    for (column, &unit) in text.iter().enumerate() {
        let (mut diagonal_cost, mut diagonal_start) = (cost[0], start[0]);
        cost[0] = 0;
        start[0] = column + 1;
        for row in 1..=rows {
            let (up_cost, up_start) = (cost[row], start[row]);
            let substitute = diagonal_cost + if pattern[row - 1] == unit { 0 } else { MISSING };
            let (mut next_cost, mut next_start) = (substitute, diagonal_start);
            if cost[row - 1] + MISSING < next_cost {
                (next_cost, next_start) = (cost[row - 1] + MISSING, start[row - 1]);
            }
            if up_cost + EXTRA < next_cost {
                (next_cost, next_start) = (up_cost + EXTRA, up_start);
            }
            (diagonal_cost, diagonal_start) = (up_cost, up_start);
            cost[row] = next_cost;
            start[row] = next_start;
        }
        if best.as_ref().is_none_or(|(_, distance)| cost[rows] < *distance) {
            best = Some((start[rows]..column + 1, cost[rows]));
        }
    }
    best
}

/// The best text like `quote` in `text`, if it's like enough: its text at
/// least 3/4 the quote's, or half and its context half the quote's.
fn approximate(text: &[u16], quote: &Quote, hint: Option<usize>) -> Option<Found> {
    let exact = &quote.exact;
    let length = exact.len();
    if length < 4 || text.is_empty() {
        return None;
    }
    let slack = (length / 2).min(256);
    // Candidate starts, from pieces of the quote and its context found as
    // they are: some piece survives any edit that leaves it alike.
    let pieces = (length / 8).clamp(1, 8);
    let piece = length / pieces;
    let mut votes: HashMap<usize, u32> = HashMap::new();
    let mut vote = |start: usize| *votes.entry(start / 8).or_default() += 1;
    for index in 0..pieces {
        for at in occurrences(text, &exact[index * piece..(index + 1) * piece], 200) {
            vote(at.saturating_sub(index * piece));
        }
    }
    if quote.prefix.len() >= 8 {
        for at in occurrences(text, &quote.prefix, 200) {
            vote(at + quote.prefix.len());
        }
    }
    if quote.suffix.len() >= 8 {
        for at in occurrences(text, &quote.suffix, 200) {
            vote(at.saturating_sub(length));
        }
    }
    let mut candidates: Vec<(usize, u32)> = votes.into_iter().collect();
    let near = |bucket: usize| hint.map_or(0, |hint| (bucket * 8).abs_diff(hint));
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then(near(a.0).cmp(&near(b.0))));
    candidates.truncate(16);
    let mut best: Option<(f32, Found)> = None;
    for (bucket, _) in candidates {
        let guess = bucket * 8;
        let Some(range) = place(text, exact, guess, slack) else { continue };
        let alike = similarity(exact, &text[range.clone()]);
        let (score, context) = score(text, quote, &range, alike, hint);
        let fits = alike >= 0.75 || (alike >= 0.5 && context.is_some_and(|context| context >= 0.5));
        if fits && best.as_ref().is_none_or(|(best, _)| score > *best) {
            best = Some((score, Found { range, similarity: alike }));
        }
    }
    best.map(|(_, found)| found)
}

/// Where `exact` is in `text` if it starts near `guess`: its start found by
/// aligning its head there, its end by aligning its tail.
fn place(text: &[u16], exact: &[u16], guess: usize, slack: usize) -> Option<Range<usize>> {
    const ENDS: usize = 64;
    let length = exact.len();
    let low = |at: usize| at.saturating_sub(slack + 8);
    let high = |at: usize| (at + slack + 8).min(text.len());
    if length <= 2 * ENDS {
        let window = low(guess)..high(guess + length);
        return within(text, window, exact).map(|(range, _)| range);
    }
    let (head, _) = within(text, low(guess)..high(guess + ENDS), &exact[..ENDS])?;
    let tail_at = guess + length - ENDS;
    let (tail, _) = within(text, low(tail_at)..high(guess + length), &exact[length - ENDS..])?;
    (head.start < tail.end).then_some(head.start..tail.end)
}

/// `align_in` on part of `text`, with the range in `text`'s offsets.
fn within(text: &[u16], window: Range<usize>, pattern: &[u16]) -> Option<(Range<usize>, usize)> {
    if window.start >= window.end {
        return None;
    }
    let (range, distance) = align_in(&text[window.clone()], pattern)?;
    Some((range.start + window.start..range.end + window.start, distance))
}

/// One stretch of a diff: old text and the new text in its place.
#[derive(Clone, Debug, PartialEq)]
struct Segment {
    old: Range<usize>,
    new: Range<usize>,
    equal: bool,
}

/// How offsets in an old text move in a new one.
#[derive(Debug)]
pub(crate) struct Mapping {
    segments: Vec<Segment>,
    old_len: usize,
    new_len: usize,
}

/// Where each line of `text` starts, and where it ends.
fn line_starts(text: &[u16]) -> Vec<usize> {
    let mut starts = vec![0];
    starts.extend(text.iter().enumerate().filter(|(_, unit)| **unit == u16::from(b'\n')).map(|(at, _)| at + 1));
    if starts.last() != Some(&text.len()) {
        starts.push(text.len());
    }
    starts
}

impl Mapping {
    /// The diff of `old` and `new`: by lines, then by units within the lines
    /// that changed.
    pub(crate) fn new(old: &[u16], new: &[u16]) -> Mapping {
        let deadline = Instant::now() + DIFF_TIME;
        let (old_starts, new_starts) = (line_starts(old), line_starts(new));
        let lines = |text: &'_ [u16], starts: &[usize]| -> Vec<Vec<u16>> {
            starts.windows(2).map(|pair| text[pair[0]..pair[1]].to_vec()).collect()
        };
        let (old_lines, new_lines) = (lines(old, &old_starts), lines(new, &new_starts));
        let ops = capture_diff_slices_deadline(Algorithm::Myers, &old_lines, &new_lines, Some(deadline));
        let mut segments = Vec::new();
        // A run of changed lines, as unit ranges.
        let mut changed: Option<(Range<usize>, Range<usize>)> = None;
        let flush = |changed: &mut Option<(Range<usize>, Range<usize>)>, segments: &mut Vec<Segment>| {
            if let Some((was, is)) = changed.take() {
                refine(&old[was.clone()], &new[is.clone()], was.start, is.start, segments);
            }
        };
        for op in ops {
            let (tag, old_lines, new_lines) = op.as_tag_tuple();
            let was = old_starts[old_lines.start]..old_starts[old_lines.end];
            let is = new_starts[new_lines.start]..new_starts[new_lines.end];
            if tag == DiffTag::Equal {
                flush(&mut changed, &mut segments);
                segments.push(Segment { old: was, new: is, equal: true });
            } else {
                changed = Some(match changed.take() {
                    Some((old_run, new_run)) => (old_run.start..was.end, new_run.start..is.end),
                    None => (was, is),
                });
            }
        }
        flush(&mut changed, &mut segments);
        Mapping { segments, old_len: old.len(), new_len: new.len() }
    }

    /// Where a range's start moves: past text inserted right there, to the
    /// start of whatever replaced text it was in.
    pub(crate) fn start(&self, at: usize) -> usize {
        if at >= self.old_len {
            return self.new_len;
        }
        let index = self.segments.partition_point(|segment| segment.old.end <= at);
        match self.segments.get(index) {
            Some(segment) if segment.equal => segment.new.start + (at - segment.old.start),
            Some(segment) => segment.new.start,
            None => self.new_len,
        }
    }

    /// Where a range's end moves: before text inserted right there, to the
    /// end of whatever replaced text it was in.
    pub(crate) fn end(&self, at: usize) -> usize {
        if at == 0 {
            return 0;
        }
        let index = self.segments.partition_point(|segment| segment.old.end < at);
        match self.segments.get(index) {
            Some(segment) if segment.equal => segment.new.start + (at - segment.old.start),
            Some(segment) => segment.new.end,
            None => self.new_len,
        }
    }
}

/// The segments of a changed stretch, diffed unit by unit (or as one
/// replacement when that takes too long or it's very big).
fn refine(old: &[u16], new: &[u16], old_at: usize, new_at: usize, segments: &mut Vec<Segment>) {
    let whole = Segment { old: old_at..old_at + old.len(), new: new_at..new_at + new.len(), equal: false };
    if old.is_empty() || new.is_empty() || old.len().saturating_mul(new.len()) > 1 << 32 {
        segments.push(whole);
        return;
    }
    let ops = capture_diff_slices_deadline(Algorithm::Myers, old, new, Some(Instant::now() + DIFF_TIME));
    for op in ops {
        let (tag, was, is) = op.as_tag_tuple();
        segments.push(Segment {
            old: old_at + was.start..old_at + was.end,
            new: new_at + is.start..new_at + is.end,
            equal: tag == DiffTag::Equal,
        });
    }
}

/// The plain text a Markdown file's formatted view shows, with runs of
/// whitespace as one space, and where each of its units comes from in the
/// file's text.
#[derive(Debug, Default)]
pub(crate) struct Projection {
    pub text: Vec<u16>,
    /// The range of the file's text (UTF-16) each unit comes from.
    from: Vec<(u32, u32)>,
}

impl Projection {
    pub(crate) fn new(source: &str) -> Projection {
        // The UTF-16 offset of each byte offset of `source`.
        let mut offsets = vec![0u32; source.len() + 1];
        let mut unit = 0u32;
        for (at, character) in source.char_indices() {
            offsets[at..at + character.len_utf8()].fill(unit);
            unit += character.len_utf16() as u32;
        }
        offsets[source.len()] = unit;
        let mut projection = Projection::default();
        let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
            match event {
                Event::Text(text) | Event::Code(text) => {
                    let written = &source[range.clone()];
                    match written.find(text.as_ref()) {
                        // Each character from where it's written.
                        Some(at) => {
                            for (offset, character) in text.char_indices() {
                                let byte = range.start + at + offset;
                                projection.push(character, offsets[byte], offsets[byte + character.len_utf8()]);
                            }
                        }
                        // Written otherwise (an entity, an escape): all from the whole.
                        None => {
                            for character in text.chars() {
                                projection.push(character, offsets[range.start], offsets[range.end]);
                            }
                        }
                    }
                }
                Event::SoftBreak | Event::HardBreak => projection.push(' ', offsets[range.start], offsets[range.end]),
                Event::End(
                    TagEnd::Paragraph
                    | TagEnd::Heading(_)
                    | TagEnd::BlockQuote(_)
                    | TagEnd::CodeBlock
                    | TagEnd::Item
                    | TagEnd::TableCell
                    | TagEnd::TableRow
                    | TagEnd::TableHead,
                ) => projection.push(' ', offsets[range.end], offsets[range.end]),
                _ => {}
            }
        }
        if projection.text.last() == Some(&u16::from(b' ')) {
            projection.text.pop();
            projection.from.pop();
        }
        projection
    }

    fn push(&mut self, character: char, start: u32, end: u32) {
        if character.is_whitespace() {
            if self.text.is_empty() || self.text.last() == Some(&u16::from(b' ')) {
                return;
            }
            self.text.push(u16::from(b' '));
            self.from.push((start, end));
            return;
        }
        let mut buffer = [0u16; 2];
        for unit in character.encode_utf16(&mut buffer) {
            self.text.push(*unit);
            self.from.push((start, end));
        }
    }

    /// The file's text a range of this text comes from; None if it's empty.
    pub(crate) fn to_source(&self, range: &Range<usize>) -> Option<Range<usize>> {
        if range.start >= range.end || range.end > self.text.len() {
            return None;
        }
        Some(self.from[range.start].0 as usize..self.from[range.end - 1].1 as usize)
    }

    /// This text's range that comes from a range of the file's text; None if
    /// none of it shows.
    pub(crate) fn shown_from(&self, range: &Range<usize>) -> Option<Range<usize>> {
        let start = self.from.partition_point(|(_, end)| (*end as usize) <= range.start);
        let end = self.from.partition_point(|(start, _)| (*start as usize) < range.end);
        (start < end).then_some(start..end)
    }
}

/// A text file, as threads on it are placed.
pub(crate) struct Doc {
    pub source: Vec<u16>,
    /// What its formatted view shows, for a Markdown file.
    pub projection: Option<Projection>,
}

/// The text a thread's quote is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Space {
    /// The file's text.
    Source,
    /// A Markdown file's formatted view.
    Rendered,
}

impl Doc {
    pub(crate) fn new(text: &str, markdown: bool) -> Doc {
        Doc { source: units(text), projection: markdown.then(|| Projection::new(text)) }
    }

    /// `space`, or the file's text when it isn't Markdown.
    fn effective(&self, space: Space) -> Space {
        if self.projection.is_some() { space } else { Space::Source }
    }

    pub(crate) fn text(&self, space: Space) -> &[u16] {
        match (self.effective(space), &self.projection) {
            (Space::Rendered, Some(projection)) => &projection.text,
            _ => &self.source,
        }
    }

    /// A range of `space` as a range of the file's text. From a Markdown
    /// file's formatted view, it takes in the inline markup around it
    /// ("**not**" for "not").
    pub(crate) fn to_source(&self, space: Space, range: &Range<usize>) -> Option<Range<usize>> {
        match (self.effective(space), &self.projection) {
            (Space::Rendered, Some(projection)) => {
                let Range { mut start, mut end } = projection.to_source(range)?;
                let markup = |unit: u16| matches!(char::from_u32(u32::from(unit)), Some('*' | '_' | '`' | '~'));
                while start > 0 && markup(self.source[start - 1]) {
                    start -= 1;
                }
                while end < self.source.len() && markup(self.source[end]) {
                    end += 1;
                }
                Some(start..end)
            }
            _ => (range.start < range.end && range.end <= self.source.len()).then(|| range.clone()),
        }
    }

    /// A range of the file's text as a range of `space`.
    pub(crate) fn in_space(&self, space: Space, range: &Range<usize>) -> Option<Range<usize>> {
        match (self.effective(space), &self.projection) {
            (Space::Rendered, Some(projection)) => projection.shown_from(range),
            _ => (range.start < range.end && range.end <= self.source.len()).then(|| range.clone()),
        }
    }

    /// The quote of a range of the file's text, in `space`.
    pub(crate) fn quote(&self, space: Space, range: &Range<usize>) -> Option<Quote> {
        let range = self.in_space(space, range)?;
        Some(Quote::of(self.text(space), &range))
    }

    /// The lines (from 1) a range of the file's text spans.
    pub(crate) fn lines(&self, range: &Range<usize>) -> (usize, usize) {
        let newline = u16::from(b'\n');
        let first = 1 + self.source[..range.start.min(self.source.len())].iter().filter(|unit| **unit == newline).count();
        let last_at = range.end.saturating_sub(1).max(range.start).min(self.source.len());
        let last = first + self.source[range.start.min(last_at)..last_at].iter().filter(|unit| **unit == newline).count();
        (first, last)
    }

    /// Where a quote made on this text in `space` is: at `hint` (a range of
    /// `space`) if it's there as it is, else wherever it fits best. The
    /// range of the file's text, and how alike the text there is.
    pub(crate) fn place(&self, space: Space, quote: &Quote, hint: Option<&Range<usize>>) -> Option<(Range<usize>, f32)> {
        let text = self.text(space);
        if let Some(hint) = hint
            && hint.end <= text.len()
            && text[hint.clone()] == *quote.exact
            && let Some(range) = self.to_source(space, hint)
        {
            return Some((range, 1.0));
        }
        let found = find(text, quote, hint.map(|hint| hint.start))?;
        Some((self.to_source(space, &found.range)?, found.similarity))
    }

    /// Where a range of the text this file was (its quote in `space`:
    /// `quote`) is in it now, `mapping` leading from that text: mapped through the diff if the text
    /// there is still like its quote, else found again. The range of this
    /// file's text, and how alike the text there is to the quote.
    pub(crate) fn follow(
        &self,
        mapping: &Mapping,
        space: Space,
        range: Option<&Range<usize>>,
        quote: &Quote,
    ) -> Option<(Range<usize>, f32)> {
        let mut hint = None;
        if let Some(range) = range {
            let mapped = mapping.start(range.start)..mapping.end(range.end);
            if let Some(there) = self.in_space(space, &mapped) {
                let alike = similarity(&self.text(space)[there.clone()], &quote.exact);
                if alike >= 0.9 {
                    return Some((self.to_source(space, &there)?, alike));
                }
                hint = Some(there.start);
            }
        }
        let found = find(self.text(space), quote, hint)?;
        Some((self.to_source(space, &found.range)?, found.similarity))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str, quote: &Quote, hint: Option<usize>) -> Option<String> {
        let text = units(text);
        find(&text, quote, hint).map(|found| string(&text[found.range]))
    }

    #[test]
    fn a_quote_is_found_by_its_context_and_then_approximately() {
        let text = "The cat sat. The cat ran. The dog sat.";
        assert_eq!(found(text, &Quote::new("cat", "The ", " ran"), None).as_deref(), Some("cat"));
        let second = find(&units(text), &Quote::new("cat", "The ", " ran"), None).expect("found");
        assert_eq!(second.range, 17..20, "the copy whose context fits");
        let nearest = find(&units(text), &Quote::new("cat", "", ""), Some(15)).expect("found");
        assert_eq!(nearest.range, 17..20, "without context, the one nearest the hint");

        let essay = "The industrial revolution was not a single event but a long series of changes in how goods were made.";
        let quote = Quote::new("was not a single event but a long series", "industrial revolution ", " of changes");
        let edited = essay.replace("a single event", "one event");
        let hit = found(&edited, &quote, None).expect("an edited quote is still found");
        assert_eq!(hit, "was not one event but a long series");
        assert_eq!(found("Nothing like it here at all, really.", &quote, None), None, "unlike text isn't taken for it");
    }

    #[test]
    fn offsets_follow_the_text_through_a_diff() {
        let old = units("one\ntwo three\nfour\n");
        let new = units("zero\none\ntwo 3 three\nfour\n");
        let mapping = Mapping::new(&old, &new);
        let three = 8..13;
        assert_eq!(string(&old[three.clone()]), "three");
        assert_eq!(string(&new[mapping.start(three.start)..mapping.end(three.end)]), "three");
        assert_eq!(string(&new[mapping.start(0)..mapping.end(3)]), "one", "an insertion before it moves it");
        let two = 4..7;
        assert_eq!(string(&new[mapping.start(two.start)..mapping.end(two.end)]), "two", "an insertion after it leaves it be");
        let gone = Mapping::new(&units("abc def ghi"), &units("abc ghi"));
        assert_eq!(gone.start(4)..gone.end(7), 4..4, "deleted text maps to where it was");
    }

    #[test]
    fn markdown_shows_as_plain_text_that_maps_back_to_its_source() {
        let source = "# Title\n\nSome **bold** and `code`,\nwrapped.\n\n- one &amp; two\n";
        let projection = Projection::new(source);
        assert_eq!(string(&projection.text), "Title Some bold and code, wrapped. one & two");
        let text = string(&projection.text);
        let bold = text.find("bold").expect("bold");
        let range = projection.to_source(&(bold..bold + 4)).expect("in the source");
        assert_eq!(&source[range.clone()], "bold");
        assert_eq!(projection.shown_from(&range), Some(bold..bold + 4));
        let doc = Doc::new(source, true);
        let quote = Quote::new("and code, wrapped", "", "").collapsed();
        let (range, alike) = doc.place(Space::Rendered, &quote, None).expect("placed");
        assert_eq!((&source[range.clone()], alike), ("and `code`,\nwrapped", 1.0));
        assert_eq!(doc.lines(&range), (3, 4));
    }

    #[test]
    fn a_thread_follows_its_text_when_the_file_is_rewritten() {
        let old = Doc::new("Intro.\n\nThe point is here.\n\nEnd.\n", false);
        let new = Doc::new("A new intro, longer.\n\nMore.\n\nThe point is here.\n\nEnd.\n", false);
        let mapping = Mapping::new(&old.source, &new.source);
        let at = 8..26;
        let quote = Quote::of(&old.source, &at);
        let (range, alike) = new.follow(&mapping, Space::Source, Some(&at), &quote).expect("followed");
        assert_eq!((string(&new.source[range]), alike), ("The point is here.".to_owned(), 1.0));
        let reworded = Doc::new("Intro.\n\nEnd.\n\nThe point is right here.\n", false);
        let mapping = Mapping::new(&old.source, &reworded.source);
        let (range, alike) = reworded.follow(&mapping, Space::Source, Some(&at), &quote).expect("found again");
        assert_eq!(string(&reworded.source[range]), "The point is right here.");
        assert!(alike < 1.0);
        let gone = Doc::new("Something else entirely.\n", false);
        let mapping = Mapping::new(&old.source, &gone.source);
        assert_eq!(gone.follow(&mapping, Space::Source, Some(&at), &quote), None);
    }
}
