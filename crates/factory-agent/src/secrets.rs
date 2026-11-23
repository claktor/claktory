//! Secrets for agents' commands. A bash call names the secrets it needs;
//! the program running the agent resolves them (only those granted to the
//! agent) into variables set in that command's environment alone. Values
//! never reach the model: every tool result, streamed or final, passes
//! through a redactor that hides every value it knows, raw or encoded.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use aho_corasick::{AhoCorasick, MatchKind};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use parking_lot::RwLock;
use serde_json::Value;
use zeroize::Zeroizing;

use crate::stream::BoxFuture;
use crate::types::{ToolResult, ToolResultPart};

/// Values shorter than this many bytes aren't hidden: they'd mask ordinary
/// text everywhere.
pub const MIN_REDACTED: usize = 8;

/// One variable a secret sets for a command.
#[derive(Clone)]
pub struct SecretVariable {
    /// The secret's name: "AWS".
    pub secret: String,
    /// The environment variable: "AWS_SECRET_ACCESS_KEY".
    pub variable: String,
    pub value: Zeroizing<String>,
    /// The value goes in a file (mode 0600, removed when the command ends)
    /// and the variable holds its path, as KUBECONFIG does.
    pub file: bool,
    /// Not secret (an AWS region beside its keys): set like the rest, but
    /// never hidden.
    pub public: bool,
    /// A file's bytes that aren't text (an imported .p12), held as standard
    /// base64 in `value`: the file gets the bytes themselves, and what's
    /// hidden is the bytes' forms. Only ever with `file`.
    pub binary: bool,
}

impl SecretVariable {
    /// The value's bytes: decoded for a binary one.
    pub fn bytes(&self) -> Zeroizing<Vec<u8>> {
        value_bytes(&self.value, self.binary)
    }
}

/// `value`'s bytes: itself, or for a binary one its base64 decoded (empty
/// if it isn't base64, which the vault never stores).
fn value_bytes(value: &str, binary: bool) -> Zeroizing<Vec<u8>> {
    if binary {
        Zeroizing::new(base64::engine::general_purpose::STANDARD.decode(value.as_bytes()).unwrap_or_default())
    } else {
        Zeroizing::new(value.as_bytes().to_vec())
    }
}

impl fmt::Debug for SecretVariable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretVariable")
            .field("secret", &self.secret)
            .field("variable", &self.variable)
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

/// Resolves the secrets a command asks for: given the agent's id and the
/// names, every variable of those secrets, or why not (not granted, the
/// vault is locked…), which the agent reads as the call's error.
pub type Resolve = Arc<dyn Fn(String, Vec<String>) -> BoxFuture<Result<Vec<SecretVariable>, String>> + Send + Sync>;

/// A command that ran with secrets, for the vault's log.
#[derive(Clone, Debug, PartialEq)]
pub struct SecretUse {
    pub agent: String,
    /// The secrets' names, as asked.
    pub secrets: Vec<String>,
    pub command: String,
    /// The background task it runs as ("task-12", the id the agent's tasks
    /// are recorded under), when it does: started in the background, or
    /// moved there. None for a command that ran in the foreground to its end.
    pub task: Option<String>,
    /// When it started, in ms since 1970.
    pub started: u64,
}

/// Hears about each command that ran with secrets, once: a background
/// command when it starts; a foreground one when it ends, or moves to the
/// background (then with its task).
pub type Used = Arc<dyn Fn(SecretUse) + Send + Sync>;

/// What an agent's bash needs for secrets, from the program running it.
#[derive(Clone)]
pub struct Secrets {
    /// The agent's id, given to `resolve`.
    pub agent: String,
    pub resolve: Resolve,
    pub used: Used,
    /// Hides values in what the agent sees.
    pub redaction: Redaction,
    /// The program, and its arguments, that a command with secrets writes
    /// its log through, so the log holds no value: one that runs
    /// `filter_main`. Empty: such logs hold what the command printed.
    pub filter: Vec<std::ffi::OsString>,
}

/// Hides secret values in text: each value of at least `MIN_REDACTED`
/// bytes, raw, line by line (PEM keys), in base64 (standard and URL-safe, at
/// each of the 3 alignments it can have inside longer data, without
/// padding), hex (lower and upper case), percent-encoded and JSON-escaped,
/// becomes "[secret AWS/AWS_SECRET_ACCESS_KEY]". Forms of `LONG` bytes or
/// more (a large file's) are found by their pieces instead: any part of one
/// at least 2 × `PIECE` long is hidden, the whole of it too.
#[derive(Clone, Default)]
pub struct Redactor {
    /// What it was built from, so it can be extended (`with`): each value's
    /// label, bytes, and whether they're text.
    values: Arc<Vec<Hidden>>,
    /// The short forms; None when there are none.
    automaton: Option<AhoCorasick>,
    /// Each pattern's replacement.
    replacements: Arc<Vec<Arc<str>>>,
    /// The same patterns, to tell whether the end of a stream could still
    /// become one (`undecided`).
    prefixes: Option<Arc<aho_corasick::nfa::contiguous::NFA>>,
    /// The long forms.
    pieces: Arc<Pieces>,
    /// The longest pattern, in bytes.
    longest: usize,
}

impl fmt::Debug for Redactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Redactor")
            .field("values", &self.values.len())
            .field("patterns", &self.replacements.len())
            .field("long", &self.pieces.forms.len())
            .finish()
    }
}

/// Forms at least this long aren't patterns of the automaton, whose size
/// grows with every byte of every pattern (a 1 MB file's forms took GBs and
/// tens of seconds to build): they're found by their pieces.
const LONG: usize = 4096;

/// A long form is indexed by its pieces this long, at each multiple of it,
/// so any part of it at least 2 × PIECE − 1 long holds a whole one.
const PIECE: usize = 32;

/// Pieces with fewer different bytes than this (zeros, "AAAA…") aren't
/// indexed: they'd hide the same bytes anywhere.
const PLAIN: usize = 8;

/// The rolling hash's base.
const BASE: u64 = 0x0100_0000_01b3;

/// A piece's hash: the polynomial of its bytes (each + 1) in `BASE`.
fn piece_hash(piece: &[u8]) -> u64 {
    piece.iter().fold(0u64, |hash, &byte| hash.wrapping_mul(BASE).wrapping_add(u64::from(byte) + 1))
}

/// The forms of `LONG` bytes or more, and an index of their pieces.
#[derive(Default)]
struct Pieces {
    forms: Vec<(Zeroizing<Vec<u8>>, Arc<str>)>,
    /// (the piece's hash, its form, its offset in it), sorted.
    index: Vec<(u64, u32, u32)>,
}

/// A stretch of text to hide.
struct Span<'r> {
    start: usize,
    end: usize,
    replacement: &'r str,
    /// A long form's part that reaches the end of the text before the
    /// form's own end: more of it may follow in a stream.
    open: bool,
}

impl Pieces {
    /// Indexes `form`, or gives it back when every piece of it is plain.
    fn add(&mut self, form: Zeroizing<Vec<u8>>, replacement: Arc<str>) -> Option<(Zeroizing<Vec<u8>>, Arc<str>)> {
        let id = self.forms.len() as u32;
        let before = self.index.len();
        for offset in (0..=form.len() - PIECE).step_by(PIECE) {
            let piece = &form[offset..offset + PIECE];
            let mut seen = [false; 256];
            let distinct = piece.iter().filter(|&&byte| !std::mem::replace(&mut seen[usize::from(byte)], true)).count();
            if distinct >= PLAIN {
                self.index.push((piece_hash(piece), id, offset as u32));
            }
        }
        if self.index.len() == before {
            return Some((form, replacement));
        }
        self.forms.push((form, replacement));
        None
    }

    fn is_empty(&self) -> bool {
        self.forms.is_empty()
    }

    /// Each stretch of `bytes` that's part of a long form and holds one of
    /// its indexed pieces, as long as it goes.
    fn runs<'r>(&'r self, bytes: &[u8]) -> Vec<Span<'r>> {
        let mut runs = Vec::new();
        if self.forms.is_empty() || bytes.len() < PIECE {
            return runs;
        }
        let power = (1..PIECE).fold(1u64, |power, _| power.wrapping_mul(BASE));
        let mut hash = piece_hash(&bytes[..PIECE]);
        // The run being read through, and its form: that form's pieces in
        // it are the same run.
        let mut current: Option<(usize, u32)> = None;
        for at in 0..=bytes.len() - PIECE {
            if at > 0 {
                let gone = (u64::from(bytes[at - 1]) + 1).wrapping_mul(power);
                hash = hash.wrapping_sub(gone).wrapping_mul(BASE).wrapping_add(u64::from(bytes[at + PIECE - 1]) + 1);
            }
            let first = self.index.partition_point(|entry| entry.0 < hash);
            let mut best: Option<(Span<'r>, u32)> = None;
            for &(_, id, offset) in self.index[first..].iter().take_while(|entry| entry.0 == hash) {
                if current.is_some_and(|(end, form)| at < end && form == id) {
                    continue;
                }
                let (form, replacement) = &self.forms[id as usize];
                let offset = offset as usize;
                if form[offset..offset + PIECE] != bytes[at..at + PIECE] {
                    continue;
                }
                let (mut start, mut from) = (at, offset);
                while start > 0 && from > 0 && bytes[start - 1] == form[from - 1] {
                    start -= 1;
                    from -= 1;
                }
                let (mut end, mut to) = (at + PIECE, offset + PIECE);
                while end < bytes.len() && to < form.len() && bytes[end] == form[to] {
                    end += 1;
                    to += 1;
                }
                let open = end == bytes.len() && to < form.len();
                if best.as_ref().is_none_or(|(span, _)| end - start > span.end - span.start) {
                    best = Some((Span { start, end, replacement, open }, id));
                }
            }
            if let Some((span, id)) = best {
                if current.is_none_or(|(end, _)| span.end > end) {
                    current = Some((span.end, id));
                }
                runs.push(span);
            }
        }
        runs
    }

    /// Whether `bytes` is, whole, part of a long form.
    fn covers(&self, bytes: &[u8]) -> bool {
        self.runs(bytes).iter().any(|run| run.start == 0 && run.end == bytes.len())
    }
}

impl Redactor {
    /// Hides every value of `variables`. Build it from every value there is,
    /// not only those an agent was given, so a value it found some other way
    /// is hidden too.
    pub fn new(variables: &[SecretVariable]) -> Redactor {
        Redactor::build(variables.iter().filter(|variable| !variable.public).map(entry).collect())
    }

    /// This redactor, also hiding `variables`: itself when it hides them
    /// already.
    pub fn with(&self, variables: &[SecretVariable]) -> Redactor {
        let new: Vec<_> = variables
            .iter()
            .filter(|variable| !variable.public)
            .map(entry)
            .filter(|new| !self.values.iter().any(|known| known.0 == new.0 && known.2 == new.2 && *known.1 == *new.1))
            .collect();
        if new.is_empty() {
            return self.clone();
        }
        let mut values = (*self.values).clone();
        values.extend(new);
        Redactor::build(values)
    }

    fn build(values: Vec<Hidden>) -> Redactor {
        let mut short: Vec<(Zeroizing<Vec<u8>>, Arc<str>)> = Vec::new();
        let mut pieces = Pieces::default();
        let mut seen = hashbrown::HashSet::new();
        for (label, value, text) in &values {
            let replacement: Arc<str> = format!("[secret {label}]").into();
            for form in forms(value, *text) {
                if form.len() < MIN_REDACTED || !seen.insert(form.to_vec()) {
                    continue;
                }
                if form.len() < LONG {
                    short.push((form, replacement.clone()));
                } else if let Some(plain) = pieces.add(form, replacement.clone()) {
                    short.push(plain);
                }
            }
        }
        // `seen` held copies of the patterns: wipe them too.
        for mut copy in seen.drain() {
            zeroize::Zeroize::zeroize(&mut copy);
        }
        pieces.index.sort_unstable();
        // What the pieces find whole anyway (a long value's lines) needn't
        // be a pattern too.
        if !pieces.is_empty() {
            short.retain(|(form, _)| form.len() < 2 * PIECE - 1 || !pieces.covers(form));
        }
        let (patterns, replacements): (Vec<Zeroizing<Vec<u8>>>, Vec<Arc<str>>) = short.into_iter().unzip();
        let longest = patterns.iter().map(|pattern| pattern.len()).max().unwrap_or(0);
        let automaton = if patterns.is_empty() {
            None
        } else {
            match AhoCorasick::builder().match_kind(MatchKind::LeftmostLongest).build(patterns.iter().map(|pattern| pattern.as_slice())) {
                Ok(automaton) => Some(automaton),
                Err(error) => {
                    tracing::error!(%error, "could not build the secrets redactor");
                    None
                }
            }
        };
        let prefixes = if patterns.is_empty() {
            None
        } else {
            aho_corasick::nfa::contiguous::NFA::new(patterns.iter().map(|pattern| pattern.as_slice())).ok().map(Arc::new)
        };
        Redactor { values: Arc::new(values), automaton, replacements: Arc::new(replacements), prefixes, pieces: Arc::new(pieces), longest }
    }

    /// Where the end of `bytes` could still turn into a value it hides,
    /// once more comes: the start of the longest end that's the beginning of
    /// one of its patterns, or `bytes.len()` when there's none. A stream
    /// holds back only that, and with long forms its last 2 × `PIECE`
    /// bytes, which could be the start of one.
    fn undecided(&self, bytes: &[u8]) -> usize {
        use aho_corasick::Anchored;
        use aho_corasick::automaton::Automaton;
        let held = if self.pieces.is_empty() { bytes.len() } else { bytes.len().saturating_sub(2 * PIECE) };
        let Some(prefixes) = &self.prefixes else { return held };
        let Ok(start) = prefixes.start_state(Anchored::Yes) else {
            return held.min(bytes.len().saturating_sub(self.longest.saturating_sub(1)));
        };
        let earliest = bytes.len().saturating_sub(self.longest.saturating_sub(1));
        (earliest..bytes.len())
            .find(|&from| {
                let mut state = start;
                bytes[from..].iter().all(|&byte| {
                    state = prefixes.next_state(Anchored::Yes, state, byte);
                    !prefixes.is_dead(state)
                })
            })
            .unwrap_or(bytes.len())
            .min(held)
    }

    /// Whether it hides anything.
    pub fn is_empty(&self) -> bool {
        self.automaton.is_none() && self.pieces.is_empty()
    }

    /// What to hide in `bytes`, in order and apart. `text`: `bytes` is
    /// UTF-8, and each stretch is widened to whole characters.
    fn spans(&self, bytes: &[u8], text: bool) -> Vec<Span<'_>> {
        let mut spans: Vec<Span<'_>> = match &self.automaton {
            Some(automaton) => automaton
                .find_iter(bytes)
                .map(|found| Span {
                    start: found.start(),
                    end: found.end(),
                    replacement: &self.replacements[found.pattern().as_usize()],
                    open: false,
                })
                .collect(),
            None => Vec::new(),
        };
        if self.pieces.is_empty() && !text {
            return spans;
        }
        spans.extend(self.pieces.runs(bytes));
        if text {
            for span in &mut spans {
                while span.start > 0 && bytes[span.start] & 0xC0 == 0x80 {
                    span.start -= 1;
                }
                while span.end < bytes.len() && bytes[span.end] & 0xC0 == 0x80 {
                    span.end += 1;
                }
            }
        }
        spans.sort_by_key(|span| (span.start, std::cmp::Reverse(span.end)));
        let mut merged: Vec<Span<'_>> = Vec::with_capacity(spans.len());
        for span in spans {
            match merged.last_mut() {
                Some(last) if span.start < last.end => {
                    if span.end > last.end {
                        last.end = span.end;
                        last.open = span.open;
                    } else if span.end == last.end {
                        last.open |= span.open;
                    }
                }
                _ => merged.push(span),
            }
        }
        merged
    }

    /// `text` with every value hidden; borrowed when there was none.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if self.is_empty() {
            return Cow::Borrowed(text);
        }
        let spans = self.spans(text.as_bytes(), true);
        if spans.is_empty() {
            return Cow::Borrowed(text);
        }
        let mut out = String::with_capacity(text.len());
        let mut last = 0;
        for span in spans {
            out.push_str(&text[last..span.start]);
            out.push_str(span.replacement);
            last = span.end;
        }
        out.push_str(&text[last..]);
        Cow::Owned(out)
    }

    /// `bytes` with every value hidden; borrowed when there was none.
    pub fn redact_bytes<'a>(&self, bytes: &'a [u8]) -> Cow<'a, [u8]> {
        if self.is_empty() {
            return Cow::Borrowed(bytes);
        }
        let spans = self.spans(bytes, false);
        if spans.is_empty() {
            return Cow::Borrowed(bytes);
        }
        let mut out = Vec::with_capacity(bytes.len());
        let mut last = 0;
        for span in spans {
            out.extend_from_slice(&bytes[last..span.start]);
            out.extend_from_slice(span.replacement.as_bytes());
            last = span.end;
        }
        out.extend_from_slice(&bytes[last..]);
        Cow::Owned(out)
    }

    /// The text parts of a tool result, and every string in its details.
    pub fn redact_result(&self, result: &mut ToolResult) {
        if self.is_empty() {
            return;
        }
        for part in &mut result.content {
            if let ToolResultPart::Text { text } = part
                && let Cow::Owned(redacted) = self.redact(text)
            {
                *text = redacted;
            }
        }
        self.redact_value(&mut result.details);
    }

    /// Every string in a JSON value.
    pub fn redact_value(&self, value: &mut Value) {
        match value {
            Value::String(text) => {
                if let Cow::Owned(redacted) = self.redact(text) {
                    *text = redacted;
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|item| self.redact_value(item)),
            Value::Object(fields) => fields.values_mut().for_each(|field| self.redact_value(field)),
            _ => {}
        }
    }

    /// The longest form of a value it hides, in bytes: a stream holds back
    /// one less, so a value split across chunks is still caught.
    pub fn longest(&self) -> usize {
        if self.pieces.is_empty() { self.longest } else { self.longest.max(2 * PIECE) }
    }
}

/// A value as the redactor keeps it: its label, bytes, and whether they're
/// text.
type Hidden = (Arc<str>, Zeroizing<Vec<u8>>, bool);

/// A variable as the redactor keeps it.
fn entry(variable: &SecretVariable) -> Hidden {
    (format!("{}/{}", variable.secret, variable.variable).into(), variable.bytes(), !variable.binary)
}

/// Every form of a value the redactor hides, short ones included (the
/// caller drops those). Bytes that aren't text (`text` false) are hidden
/// raw, in base64 and in hex: their lines, percent-encoding and JSON
/// escapes mean nothing.
fn forms(value: &[u8], text: bool) -> Vec<Zeroizing<Vec<u8>>> {
    if !text {
        let mut forms = Vec::new();
        if value.len() >= MIN_REDACTED {
            forms.push(Zeroizing::new(value.to_vec()));
            for engine in [&STANDARD_NO_PAD, &URL_SAFE_NO_PAD] {
                for offset in 0..3 {
                    forms.push(base64_aligned(engine, value, offset));
                }
            }
            // Base64 as tools wrap it (openssl's 64 columns, MIME's 76):
            // each whole line, and the last if it's long enough.
            let encoded = Zeroizing::new(base64::engine::general_purpose::STANDARD.encode(value));
            for width in [64, 76] {
                if encoded.len() > width {
                    for line in encoded.as_bytes().chunks(width) {
                        forms.push(Zeroizing::new(line.to_vec()));
                    }
                }
            }
            forms.push(hex_of(value, true));
            forms.push(hex_of(value, false));
        }
        return forms;
    }
    let mut bases: Vec<&[u8]> = vec![value];
    let trimmed = value.trim_ascii();
    if trimmed.len() != value.len() {
        bases.push(trimmed);
    }
    let mut forms = Vec::new();
    for base in bases {
        if base.len() < MIN_REDACTED {
            continue;
        }
        forms.push(Zeroizing::new(base.to_vec()));
        if base.contains(&b'\n') {
            // A PEM key's armor ("-----BEGIN OPENSSH PRIVATE KEY-----") is
            // the same in every key and says nothing secret: hiding it would
            // hide it in any output, named after whichever key came first.
            let lines = base.split(|byte| *byte == b'\n').map(<[u8]>::trim_ascii).filter(|line| !is_armor(line));
            forms.extend(lines.map(|line| Zeroizing::new(line.to_vec())));
        }
        for engine in [&STANDARD_NO_PAD, &URL_SAFE_NO_PAD] {
            for offset in 0..3 {
                forms.push(base64_aligned(engine, base, offset));
            }
        }
        forms.push(hex_of(base, true));
        forms.push(hex_of(base, false));
        if !base.iter().all(|byte| unreserved(*byte)) {
            forms.push(percent_encoded(base, true));
            forms.push(percent_encoded(base, false));
        }
        if let Ok(text) = std::str::from_utf8(base)
            && let Ok(json) = serde_json::to_string(text).map(Zeroizing::new)
        {
            let inner = &json.as_bytes()[1..json.len() - 1];
            if inner != base {
                forms.push(Zeroizing::new(inner.to_vec()));
            }
        }
    }
    forms
}

/// `value` in hex, upper or lower case.
fn hex_of(value: &[u8], upper: bool) -> Zeroizing<Vec<u8>> {
    let digits: &[u8; 16] = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let mut out = Zeroizing::new(Vec::with_capacity(value.len() * 2));
    for byte in value {
        out.push(digits[usize::from(byte >> 4)]);
        out.push(digits[usize::from(byte & 15)]);
    }
    out
}

/// A PEM armor line: "-----BEGIN …-----" or "-----END …-----".
fn is_armor(line: &[u8]) -> bool {
    (line.starts_with(b"-----BEGIN ") || line.starts_with(b"-----END ")) && line.ends_with(b"-----")
}

/// The base64 characters that depend only on `value`, when it sits
/// `offset` bytes (0, 1 or 2) after a 3-byte boundary of longer data.
fn base64_aligned(engine: &base64::engine::GeneralPurpose, value: &[u8], offset: usize) -> Zeroizing<Vec<u8>> {
    let mut input = Zeroizing::new(vec![0u8; offset]);
    input.extend_from_slice(value);
    let encoded = Zeroizing::new(engine.encode(input.as_slice()));
    let first = (8 * offset).div_ceil(6);
    let last = (8 * (offset + value.len())) / 6;
    Zeroizing::new(encoded.as_bytes()[first..last.min(encoded.len())].to_vec())
}

fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"-._~".contains(&byte)
}

/// Percent-encoding with every byte but RFC 3986's unreserved ones escaped,
/// in upper or lower case hex.
fn percent_encoded(value: &[u8], upper: bool) -> Zeroizing<Vec<u8>> {
    let mut out = Zeroizing::new(Vec::with_capacity(value.len() * 3));
    for byte in value {
        if unreserved(*byte) {
            out.push(*byte);
        } else if upper {
            out.extend_from_slice(format!("%{byte:02X}").as_bytes());
        } else {
            out.extend_from_slice(format!("%{byte:02x}").as_bytes());
        }
    }
    out
}

/// Redacts output that arrives in chunks: it holds back an end of what came
/// so far that could still become a value it hides, until more comes or the
/// stream ends, so a value split across chunks is still caught; the rest
/// goes on at once.
#[derive(Debug)]
pub struct StreamRedactor {
    redactor: Arc<Redactor>,
    pending: Vec<u8>,
    /// What it holds is the end of a long form's part whose replacement
    /// went out already.
    inside: bool,
}

impl StreamRedactor {
    pub fn new(redactor: Arc<Redactor>) -> StreamRedactor {
        StreamRedactor { redactor, pending: Vec::new(), inside: false }
    }

    /// What can be shown of the text so far, redacted.
    pub fn push(&mut self, text: &str) -> String {
        if self.redactor.is_empty() {
            return text.to_owned();
        }
        self.pending.extend_from_slice(text.as_bytes());
        // Only whole characters go in and come out, so it stays UTF-8.
        utf8(self.advance(true))
    }

    /// What can be written of the bytes so far, redacted.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<u8> {
        if self.redactor.is_empty() {
            return bytes.to_vec();
        }
        self.pending.extend_from_slice(bytes);
        self.advance(false)
    }

    /// Takes off what can be shown: everything but an end that could still
    /// become a value, and never part of a value it found. `text`: cut only
    /// between characters.
    fn advance(&mut self, text: bool) -> Vec<u8> {
        if self.redactor.is_empty() {
            return std::mem::take(&mut self.pending);
        }
        let mut cut = self.redactor.undecided(&self.pending);
        while text && cut > 0 && cut < self.pending.len() && self.pending[cut] & 0xC0 == 0x80 {
            cut -= 1;
        }
        self.take(cut, text)
    }

    /// Takes off what's before `cut` (and any value it found that starts
    /// there), redacted. A long form's part that may go on is hidden at
    /// once, and only its last 2 × `PIECE` bytes are held, so what comes
    /// next is still known for what it is; the replacement isn't repeated
    /// as the part goes on.
    fn take(&mut self, mut cut: usize, text: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut last = 0;
        let mut inside = false;
        for (index, span) in self.redactor.spans(&self.pending, text).into_iter().enumerate() {
            if span.start >= cut {
                break;
            }
            out.extend_from_slice(&self.pending[last..span.start]);
            let shown = index == 0 && span.start == 0 && self.inside;
            if span.open && cut < self.pending.len() {
                let mut held = span.end.saturating_sub(2 * PIECE);
                while text && held > span.start && self.pending[held] & 0xC0 == 0x80 {
                    held -= 1;
                }
                if held <= span.start {
                    cut = span.start;
                    inside = shown;
                    break;
                }
                if !shown {
                    out.extend_from_slice(span.replacement.as_bytes());
                }
                last = held;
                cut = held;
                inside = true;
                break;
            }
            if !shown {
                out.extend_from_slice(span.replacement.as_bytes());
            }
            last = span.end;
            cut = cut.max(span.end);
        }
        out.extend_from_slice(&self.pending[last..cut.max(last)]);
        self.pending.drain(..cut.max(last));
        self.inside = inside;
        out
    }

    /// The rest, at the end of the stream, redacted.
    pub fn finish(&mut self) -> String {
        if self.redactor.is_empty() {
            return utf8(std::mem::take(&mut self.pending));
        }
        utf8(self.take(self.pending.len(), true))
    }

    /// The rest, at the end of a stream of bytes, redacted.
    pub fn finish_bytes(&mut self) -> Vec<u8> {
        if self.redactor.is_empty() {
            return std::mem::take(&mut self.pending);
        }
        self.take(self.pending.len(), false)
    }

    /// What it's holding back, unredacted, in bytes.
    pub fn held(&self) -> usize {
        self.pending.len()
    }
}

fn utf8(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

/// The environment variable that gives `filter_main` the values to hide: a
/// JSON list of ["secret/VARIABLE", value] pairs. A command's own secrets
/// are already in its environment; the filter's copy is unset for the
/// command itself.
pub const FILTER_VALUES: &str = "CLAKTORY_REDACT";

/// Marks a binary value's label in `FILTER_VALUES` (its value is base64):
/// no secret's name can hold it.
const BINARY_LABEL: char = '\u{1}';

/// The values `filter_main` gets, for a command's secrets.
pub(crate) fn filter_values(variables: &[SecretVariable]) -> Zeroizing<String> {
    let pairs: Vec<(String, &str)> = variables
        .iter()
        .filter(|variable| !variable.public)
        .map(|variable| {
            let mark = if variable.binary { BINARY_LABEL.to_string() } else { String::new() };
            (format!("{mark}{}/{}", variable.secret, variable.variable), variable.value.as_str())
        })
        .collect();
    Zeroizing::new(serde_json::to_string(&pairs).unwrap_or_default())
}

/// Copies standard input to standard output with the values in
/// `FILTER_VALUES` hidden, as they stream: what a background command with
/// secrets writes to its log goes through it, so no value lands on disk.
/// It's a process of its own, beside the command, so the log keeps being
/// written while the program running the agents restarts. Run it from that
/// program's own executable (factoryd's `redact`).
pub fn filter_main() -> std::process::ExitCode {
    let values = Zeroizing::new(std::env::var(FILTER_VALUES).unwrap_or_default());
    match filter(std::io::stdin().lock(), std::io::stdout().lock(), &values) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}

/// Copies `input` to `output` as it comes, with `values` (as in
/// `FILTER_VALUES`) hidden.
pub fn filter(mut input: impl std::io::Read, mut output: impl std::io::Write, values: &str) -> std::io::Result<()> {
    let pairs: Vec<(String, String)> = serde_json::from_str(values).unwrap_or_default();
    let values = pairs
        .into_iter()
        .map(|(label, value)| {
            let value = Zeroizing::new(value);
            match label.strip_prefix(BINARY_LABEL) {
                Some(label) => (Arc::from(label), value_bytes(&value, true), false),
                None => (Arc::from(label.as_str()), Zeroizing::new(value.as_bytes().to_vec()), true),
            }
        })
        .collect();
    let redactor = Redactor::build(values);
    let mut stream = StreamRedactor::new(Arc::new(redactor));
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        output.write_all(&stream.push_bytes(&buffer[..read]))?;
        output.flush()?;
    }
    output.write_all(&stream.finish_bytes())?;
    output.flush()
}

/// The redactor in use now, shared: the program running the agents sets a
/// new one whenever the values to hide change, and every redaction after
/// that uses it.
#[derive(Clone, Default)]
pub struct Redaction(Arc<RwLock<Arc<Redactor>>>);

impl fmt::Debug for Redaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Redaction").field(&*self.current()).finish()
    }
}

impl Redaction {
    pub fn new(redactor: Redactor) -> Redaction {
        Redaction(Arc::new(RwLock::new(Arc::new(redactor))))
    }

    pub fn set(&self, redactor: Redactor) {
        *self.0.write() = Arc::new(redactor);
    }

    pub fn current(&self) -> Arc<Redactor> {
        self.0.read().clone()
    }

    pub fn redact(&self, text: &str) -> String {
        self.current().redact(text).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variable(secret: &str, variable: &str, value: &str) -> SecretVariable {
        SecretVariable {
            secret: secret.into(),
            variable: variable.into(),
            value: Zeroizing::new(value.into()),
            file: false,
            public: false,
            binary: false,
        }
    }

    const KEY: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

    fn redactor() -> Redactor {
        Redactor::new(&[variable("AWS", "AWS_SECRET_ACCESS_KEY", KEY), variable("Short", "PIN", "1234567")])
    }

    #[test]
    fn public_variables_are_set_but_never_hidden() {
        let region = SecretVariable { public: true, ..variable("AWS", "AWS_REGION", "eu-central-1") };
        let text = format!("arn:aws:s3:eu-central-1 {KEY}");
        let shown = "arn:aws:s3:eu-central-1 [secret AWS/AWS_SECRET_ACCESS_KEY]";
        assert_eq!(Redactor::new(&[variable("AWS", "AWS_SECRET_ACCESS_KEY", KEY), region.clone()]).redact(&text), shown);
        assert_eq!(Redactor::default().with(&[variable("AWS", "AWS_SECRET_ACCESS_KEY", KEY), region.clone()]).redact(&text), shown);
        assert!(!filter_values(&[region]).contains("eu-central-1"));
    }

    #[test]
    fn hides_raw_and_encoded_values() {
        let redactor = redactor();
        let hidden = "[secret AWS/AWS_SECRET_ACCESS_KEY]";
        assert_eq!(redactor.redact(&format!("key={KEY}!")), format!("key={hidden}!"));
        let hex: String = KEY.bytes().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(redactor.redact(&hex), hidden);
        assert_eq!(redactor.redact(&hex.to_uppercase()), hidden);
        let percent = KEY.replace('/', "%2F").replace('+', "%2B");
        assert_eq!(redactor.redact(&format!("?k={percent}&")), format!("?k={hidden}&"));
        let json = serde_json::to_string(&"a\"b\\c/d\ne-f-g-h").unwrap();
        let escaped = Redactor::new(&[variable("J", "J", "a\"b\\c/d\ne-f-g-h")]);
        assert!(escaped.redact(&json).contains("[secret J/J]"), "{}", escaped.redact(&json));
        // Base64 at every alignment, inside longer data.
        for prefix in ["", "x", "xy", "xyz"] {
            for engine in [&STANDARD_NO_PAD, &URL_SAFE_NO_PAD] {
                let encoded = engine.encode(format!("{prefix}{KEY} and more"));
                let redacted = redactor.redact(&encoded);
                assert!(redacted.contains(hidden), "{prefix:?}: {redacted}");
            }
        }
    }

    #[test]
    fn skips_short_values_and_borrows_when_nothing_matches() {
        let redactor = redactor();
        assert!(matches!(redactor.redact("pin 1234567 stays"), Cow::Borrowed("pin 1234567 stays")));
        let empty = Redactor::default();
        assert!(empty.is_empty());
        assert!(matches!(empty.redact(KEY), Cow::Borrowed(_)));
    }

    #[test]
    fn hides_each_line_of_a_multi_line_value() {
        let pem = "-----BEGIN KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASC\nBKcwggSjAgEAAoIBAQC7\n-----END KEY-----\n";
        let redactor = Redactor::new(&[variable("Deploy", "SSH_KEY", pem)]);
        assert_eq!(redactor.redact("got   MIIEvQIBADANBgkqhkiG9w0BAQEFAASC\n"), "got   [secret Deploy/SSH_KEY]\n");
    }

    #[test]
    fn a_pem_keys_armor_stays_and_its_body_is_hidden() {
        let deploy = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUA\nAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtz\n-----END OPENSSH PRIVATE KEY-----\n";
        let backup = "-----BEGIN OPENSSH PRIVATE KEY-----\nc3NoLWVkMjU1MTkAAAAgQ2xha3Rvcnkg\nVGVzdCBLZXkgT25seSBGYWtlIEtleQ==\n-----END OPENSSH PRIVATE KEY-----\n";
        let redactor = Redactor::new(&[variable("Deploy", "SSH_KEY", deploy), variable("Backup", "SSH_KEY", backup)]);
        // Each body line is hidden under its own key's name.
        let shown = redactor.redact(backup);
        assert_eq!(shown, "[secret Backup/SSH_KEY]");
        // Part of one, as `head -2` prints it: the armor shows, the body doesn't.
        let head = "-----BEGIN OPENSSH PRIVATE KEY-----\nc3NoLWVkMjU1MTkAAAAgQ2xha3Rvcnkg\n";
        let shown = redactor.redact(head);
        assert!(shown.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----\n"), "the armor stays");
        assert!(shown.ends_with("\n[secret Backup/SSH_KEY]\n") && !shown.contains("c3NoLWVk"), "the body is hidden");
        assert_eq!(redactor.redact("b3BlbnNzaC1rZXktdjEAAAAABG5vbmUA"), "[secret Deploy/SSH_KEY]");
        // The armor alone, as tools print it about any key, isn't a secret.
        let about = "ssh-keygen wrote -----BEGIN OPENSSH PRIVATE KEY----- to id_ed25519";
        assert!(matches!(redactor.redact(about), Cow::Borrowed(_)));
    }

    #[test]
    fn streams_catch_a_value_split_across_chunks() {
        let redactor = Arc::new(redactor());
        for split in 1..KEY.len() {
            let mut stream = StreamRedactor::new(redactor.clone());
            let mut shown = stream.push(&format!("before {}", &KEY[..split]));
            assert!(!shown.contains(&KEY[..split.min(4)]), "{split}: {shown}");
            shown.push_str(&stream.push(&format!("{} after", &KEY[split..])));
            shown.push_str(&stream.finish());
            assert_eq!(shown, "before [secret AWS/AWS_SECRET_ACCESS_KEY] after");
        }
        // Byte by byte, with multi-byte characters around.
        let mut stream = StreamRedactor::new(redactor);
        let text = format!("é{KEY}ü{KEY}");
        let mut shown = String::new();
        for character in text.chars() {
            shown.push_str(&stream.push(&character.to_string()));
            assert!(!shown.contains(&KEY[..4]), "{shown}");
        }
        shown.push_str(&stream.finish());
        assert_eq!(shown, "é[secret AWS/AWS_SECRET_ACCESS_KEY]ü[secret AWS/AWS_SECRET_ACCESS_KEY]");
    }

    #[test]
    fn streams_hold_back_only_what_could_still_become_a_value() {
        let mut stream = StreamRedactor::new(Arc::new(redactor()));
        assert_eq!(stream.push("listening on :3000\n"), "listening on :3000\n", "nothing to wait for");
        assert_eq!(stream.push("key: wJalrX"), "key: ", "the start of the key waits");
        assert_eq!(stream.held(), 6);
        assert_eq!(stream.push("Ut but not"), "wJalrXUt but not", "it wasn't the key after all");
        assert_eq!(stream.push(&format!(" {KEY}\n")), " [secret AWS/AWS_SECRET_ACCESS_KEY]\n");
        assert_eq!(stream.held(), 0);
    }

    /// Hands out its bytes a few at a time, as a pipe might.
    struct Trickle(Vec<u8>, usize);

    impl std::io::Read for Trickle {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let take = self.1.min(self.0.len()).min(buffer.len());
            buffer[..take].copy_from_slice(&self.0[..take]);
            self.0.drain(..take);
            Ok(take)
        }
    }

    #[test]
    fn the_filter_hides_values_in_bytes_as_they_stream() {
        let values = filter_values(&[variable("AWS", "AWS_SECRET_ACCESS_KEY", KEY)]);
        assert_eq!(FILTER_VALUES, "CLAKTORY_REDACT", "the task wrapper unsets it by this name");
        let mut input = format!("start {KEY} and ").into_bytes();
        input.push(0xff);
        input.extend_from_slice(b"\xff binary \x00 ");
        input.extend_from_slice(STANDARD_NO_PAD.encode(KEY).as_bytes());
        input.extend_from_slice(b" end");
        for step in [1, 3, 7, 64 * 1024] {
            let mut output = Vec::new();
            filter(Trickle(input.clone(), step), &mut output, &values).unwrap();
            let mut expected = b"start [secret AWS/AWS_SECRET_ACCESS_KEY] and \xff".to_vec();
            // The encoding's last character also depends on what follows, so it stays.
            let encoded = STANDARD_NO_PAD.encode(KEY);
            expected.extend_from_slice(b"\xff binary \x00 [secret AWS/AWS_SECRET_ACCESS_KEY]");
            expected.extend_from_slice(format!("{} end", &encoded[encoded.len() - 1..]).as_bytes());
            assert_eq!(String::from_utf8_lossy(&output), String::from_utf8_lossy(&expected), "{step} bytes at a time");
            assert_eq!(output, expected, "other bytes pass as they are");
        }
        // Without values it's a plain copy.
        let mut output = Vec::new();
        filter(Trickle(input.clone(), 5), &mut output, "").unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn a_binary_value_is_hidden_raw_in_base64_and_in_hex() {
        let bytes: Vec<u8> = (0..48u8).map(|byte| byte.wrapping_mul(37) ^ 0xa5).collect();
        let p12 = SecretVariable {
            file: true,
            binary: true,
            ..variable("Android", "UPLOAD_P12", &base64::engine::general_purpose::STANDARD.encode(&bytes))
        };
        assert_eq!(p12.bytes().as_slice(), bytes.as_slice(), "decoded for the file");
        let hidden = b"[secret Android/UPLOAD_P12]";
        let redactor = Redactor::new(std::slice::from_ref(&p12));
        let mut raw = b"before ".to_vec();
        raw.extend_from_slice(&bytes);
        assert!(redactor.redact_bytes(&raw).windows(hidden.len()).any(|window| window == hidden), "raw bytes");
        let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        assert_eq!(redactor.redact(&hex), "[secret Android/UPLOAD_P12]");
        assert_eq!(redactor.redact(&hex.to_uppercase()), "[secret Android/UPLOAD_P12]");
        for prefix in [&b""[..], b"x", b"xy"] {
            let mut data = prefix.to_vec();
            data.extend_from_slice(&bytes);
            for engine in [&STANDARD_NO_PAD, &URL_SAFE_NO_PAD] {
                let shown = redactor.redact(&engine.encode(&data)).into_owned();
                assert!(shown.contains("[secret Android/UPLOAD_P12]"), "{prefix:?}: {shown}");
            }
        }
        // Wrapped as openssl base64 and base64 -b 76 print it.
        let long: Vec<u8> = (0..300u32).map(|i| (i.wrapping_mul(151) ^ 0x5a) as u8).collect();
        let cert = SecretVariable { binary: true, ..variable("Android", "CERT", &base64::engine::general_purpose::STANDARD.encode(&long)) };
        let wrapped = Redactor::new(std::slice::from_ref(&cert));
        let encoded = base64::engine::general_purpose::STANDARD.encode(&long);
        for width in [64, 76] {
            let lines: Vec<&str> = encoded.as_bytes().chunks(width).map(|line| std::str::from_utf8(line).unwrap()).collect();
            let shown = wrapped.redact(&lines.join("\n")).into_owned();
            assert!(!lines.iter().any(|line| shown.contains(line)), "{width}: {shown}");
        }
        // A command's filter gets it marked binary, and hides the same.
        let values = filter_values(std::slice::from_ref(&p12));
        let mut output = Vec::new();
        filter(Trickle(raw.clone(), 5), &mut output, &values).unwrap();
        assert_eq!(output, [&b"before "[..], hidden].concat());
        let mut output = Vec::new();
        filter(Trickle(hex.into_bytes(), 3), &mut output, &values).unwrap();
        assert_eq!(output, hidden);
    }

    /// Bytes that look random (xorshift).
    fn noise(length: usize, mut seed: u64) -> Vec<u8> {
        (0..length)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                (seed >> 24) as u8
            })
            .collect()
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// What's between `before` and `after` is only `hidden`, once or more.
    fn only_hidden(shown: &[u8], before: &[u8], after: &[u8], hidden: &[u8]) -> bool {
        let Some(middle) = shown.strip_prefix(before).and_then(|rest| rest.strip_suffix(after)) else { return false };
        !middle.is_empty() && middle.len() % hidden.len() == 0 && middle.chunks(hidden.len()).all(|chunk| chunk == hidden)
    }

    #[test]
    fn a_large_file_is_quick_to_take_in_and_hidden_every_way() {
        let bytes = noise(1024 * 1024 - 1, 0x9e37_79b9_7f4a_7c15);
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let blob = SecretVariable { file: true, binary: true, ..variable("Big", "BLOB", &encoded) };
        let redactor = Redactor::new(std::slice::from_ref(&blob));
        // Its forms aren't the automaton's patterns, which took GBs: only
        // a wrapped encoding's short last line is.
        let automaton = redactor.automaton.as_ref().map_or(0, AhoCorasick::memory_usage);
        assert!(automaton < 64 * 1024, "{automaton} bytes");
        assert!(redactor.replacements.len() <= 2, "{redactor:?}");
        // Another value taken in, or the same again, costs no rebuild of it.
        let again = redactor.with(std::slice::from_ref(&blob));
        assert!(Arc::ptr_eq(&again.pieces, &redactor.pieces));
        let hidden = "[secret Big/BLOB]";

        // Raw, whole and in part (head -c, a hexdump's slice).
        let raw = [&b"before "[..], &bytes, b" after"].concat();
        assert_eq!(redactor.redact_bytes(&raw).as_ref(), b"before [secret Big/BLOB] after");
        let part = [&b"x"[..], &bytes[5000..5100], b"y"].concat();
        assert_eq!(redactor.redact_bytes(&part).as_ref(), b"x[secret Big/BLOB]y");
        // Hex, base64 at each alignment, and wrapped as openssl and base64 print it.
        let hexed = hex(&bytes);
        assert_eq!(redactor.redact(&format!("{hexed}\n")), format!("{hidden}\n"));
        assert_eq!(redactor.redact(&hexed.to_uppercase()), hidden);
        assert_eq!(redactor.redact(&format!("= {} =", &hexed[999..1999])), format!("= {hidden} ="));
        for prefix in [&b""[..], b"x", b"xy"] {
            for engine in [&STANDARD_NO_PAD, &URL_SAFE_NO_PAD] {
                let data = engine.encode([prefix, &bytes].concat());
                let shown = redactor.redact(&data).into_owned();
                assert!(shown.contains(hidden) && shown.len() < 100, "{prefix:?}: {}", &shown[..shown.len().min(200)]);
            }
        }
        for width in [64, 76] {
            let lines: Vec<&str> = encoded.as_bytes().chunks(width).map(|line| std::str::from_utf8(line).unwrap()).collect();
            let shown = redactor.redact(&lines.join("\n")).into_owned();
            assert!(!lines.iter().any(|line| line.len() >= MIN_REDACTED && shown.contains(line)), "{width}");
        }
        // Plain stretches of it don't hide the same bytes elsewhere.
        let zeros = format!("{} 0000000000000000000000000000000000000000000000000000000000000000", "A".repeat(80));
        assert!(matches!(redactor.redact(&zeros), Cow::Borrowed(_)));

        // Streamed, as a command prints it, at any pace: nothing of it shows,
        // and what's around it does (a smaller file, as long a form all the same).
        let bytes = noise(100 * 1024, 7);
        let blob = SecretVariable {
            file: true,
            binary: true,
            ..variable("Big", "BLOB", &base64::engine::general_purpose::STANDARD.encode(&bytes))
        };
        let redactor = Redactor::new(std::slice::from_ref(&blob));
        let raw = [&b"before "[..], &bytes, b" after"].concat();
        let values = filter_values(std::slice::from_ref(&blob));
        for step in [7, 1000, 64 * 1024] {
            let mut output = Vec::new();
            filter(Trickle(raw.clone(), step), &mut output, &values).unwrap();
            assert!(
                only_hidden(&output, b"before ", b" after", hidden.as_bytes()),
                "{step}: {}",
                String::from_utf8_lossy(&output[..output.len().min(300)])
            );
            assert!(output.len() < 200, "{step}: {} bytes", output.len());
        }
        let text = format!("hex: {} done\n", hex(&bytes));
        let mut stream = StreamRedactor::new(Arc::new(redactor.clone()));
        let mut shown = String::new();
        for chunk in text.as_bytes().chunks(333) {
            shown.push_str(&stream.push(std::str::from_utf8(chunk).unwrap()));
            assert!(stream.held() < 4 * 1024, "{} held", stream.held());
        }
        shown.push_str(&stream.finish());
        assert_eq!(shown, format!("hex: {hidden} done\n"));
    }

    #[test]
    fn a_large_text_value_hides_its_lines_and_any_long_part() {
        let lines: Vec<String> = (0..400).map(|line| hex(&noise(20, line + 1))).collect();
        let value = lines.join("\n");
        assert!(value.len() >= LONG);
        let redactor = Redactor::new(&[variable("Creds", "JSON", &value)]);
        let hidden = "[secret Creds/JSON]";
        assert_eq!(redactor.redact(&format!("{value}\n")), format!("{hidden}\n"));
        assert_eq!(redactor.redact(&format!(": {} :", lines[123])), format!(": {hidden} :"));
        let middle = &value[1000..3000];
        assert_eq!(redactor.redact(&format!("[{middle}]")), format!("[{hidden}]"));
    }

    #[test]
    fn redaction_is_shared_and_swappable() {
        let redaction = Redaction::default();
        let copy = redaction.clone();
        assert_eq!(copy.redact(KEY), KEY);
        redaction.set(redactor());
        assert_eq!(copy.redact(KEY), "[secret AWS/AWS_SECRET_ACCESS_KEY]");
        let wider = copy.current().with(&[variable("Mail", "RESEND_API_KEY", "re_123456789abcdef")]);
        assert_eq!(wider.redact(&format!("{KEY} re_123456789abcdef")), "[secret AWS/AWS_SECRET_ACCESS_KEY] [secret Mail/RESEND_API_KEY]");
    }
}
