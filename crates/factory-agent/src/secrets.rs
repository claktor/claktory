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
/// becomes "[secret AWS/AWS_SECRET_ACCESS_KEY]".
#[derive(Default)]
pub struct Redactor {
    /// What it was built from, so it can be extended (`with`): each value's
    /// label, bytes, and whether they're text.
    values: Vec<(Arc<str>, Zeroizing<Vec<u8>>, bool)>,
    /// None when there's nothing to hide.
    automaton: Option<AhoCorasick>,
    /// Each pattern's replacement.
    replacements: Vec<Arc<str>>,
    /// The same patterns, to tell whether the end of a stream could still
    /// become one (`undecided`).
    prefixes: Option<aho_corasick::nfa::contiguous::NFA>,
    /// The longest pattern, in bytes.
    longest: usize,
}

impl fmt::Debug for Redactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Redactor").field("values", &self.values.len()).field("patterns", &self.replacements.len()).finish()
    }
}

impl Redactor {
    /// Hides every value of `variables`. Build it from every value there is,
    /// not only those an agent was given, so a value it found some other way
    /// is hidden too.
    pub fn new(variables: &[SecretVariable]) -> Redactor {
        Redactor::build(
            variables
                .iter()
                .filter(|variable| !variable.public)
                .map(|variable| (format!("{}/{}", variable.secret, variable.variable).into(), variable.bytes(), !variable.binary))
                .collect(),
        )
    }

    /// This redactor, also hiding `variables`.
    pub fn with(&self, variables: &[SecretVariable]) -> Redactor {
        if variables.is_empty() {
            return Redactor::build(self.values.clone());
        }
        let mut values = self.values.clone();
        values.extend(
            variables
                .iter()
                .filter(|variable| !variable.public)
                .map(|variable| (format!("{}/{}", variable.secret, variable.variable).into(), variable.bytes(), !variable.binary)),
        );
        Redactor::build(values)
    }

    fn build(values: Vec<(Arc<str>, Zeroizing<Vec<u8>>, bool)>) -> Redactor {
        let mut patterns: Vec<Zeroizing<Vec<u8>>> = Vec::new();
        let mut replacements: Vec<Arc<str>> = Vec::new();
        let mut seen = hashbrown::HashSet::new();
        for (label, value, text) in &values {
            let replacement: Arc<str> = format!("[secret {label}]").into();
            for form in forms(value, *text) {
                if form.len() >= MIN_REDACTED && seen.insert(form.to_vec()) {
                    patterns.push(form);
                    replacements.push(replacement.clone());
                }
            }
        }
        // `seen` held copies of the patterns: wipe them too.
        for mut copy in seen.drain() {
            zeroize::Zeroize::zeroize(&mut copy);
        }
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
            aho_corasick::nfa::contiguous::NFA::new(patterns.iter().map(|pattern| pattern.as_slice())).ok()
        };
        Redactor { values, automaton, replacements, prefixes, longest }
    }

    /// Where the end of `bytes` could still turn into a value it hides,
    /// once more comes: the start of the longest end that's the beginning of
    /// one of its patterns, or `bytes.len()` when there's none. A stream
    /// holds back only that.
    fn undecided(&self, bytes: &[u8]) -> usize {
        use aho_corasick::Anchored;
        use aho_corasick::automaton::Automaton;
        let Some(prefixes) = &self.prefixes else { return bytes.len() };
        let Ok(start) = prefixes.start_state(Anchored::Yes) else { return bytes.len().saturating_sub(self.longest.saturating_sub(1)) };
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
    }

    /// Whether it hides anything.
    pub fn is_empty(&self) -> bool {
        self.automaton.is_none()
    }

    /// `text` with every value hidden; borrowed when there was none.
    pub fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let Some(automaton) = &self.automaton else { return Cow::Borrowed(text) };
        let mut out = String::new();
        let mut last = 0;
        for found in automaton.find_iter(text.as_bytes()) {
            out.push_str(&text[last..found.start()]);
            out.push_str(&self.replacements[found.pattern().as_usize()]);
            last = found.end();
        }
        if last == 0 && out.is_empty() {
            return Cow::Borrowed(text);
        }
        out.push_str(&text[last..]);
        Cow::Owned(out)
    }

    /// `bytes` with every value hidden; borrowed when there was none.
    pub fn redact_bytes<'a>(&self, bytes: &'a [u8]) -> Cow<'a, [u8]> {
        let Some(automaton) = &self.automaton else { return Cow::Borrowed(bytes) };
        let mut out = Vec::new();
        let mut last = 0;
        for found in automaton.find_iter(bytes) {
            out.extend_from_slice(&bytes[last..found.start()]);
            out.extend_from_slice(self.replacements[found.pattern().as_usize()].as_bytes());
            last = found.end();
        }
        if last == 0 {
            return Cow::Borrowed(bytes);
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
        self.longest
    }
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
            let hex = Zeroizing::new(value.iter().map(|byte| format!("{byte:02x}")).collect::<String>());
            forms.push(Zeroizing::new(hex.to_ascii_uppercase().into_bytes()));
            forms.push(Zeroizing::new(hex.as_bytes().to_vec()));
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
        let hex = Zeroizing::new(base.iter().map(|byte| format!("{byte:02x}")).collect::<String>());
        forms.push(Zeroizing::new(hex.to_ascii_uppercase().into_bytes()));
        forms.push(Zeroizing::new(hex.as_bytes().to_vec()));
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
}

impl StreamRedactor {
    pub fn new(redactor: Arc<Redactor>) -> StreamRedactor {
        StreamRedactor { redactor, pending: Vec::new() }
    }

    /// What can be shown of the text so far, redacted.
    pub fn push(&mut self, text: &str) -> String {
        if self.redactor.automaton.is_none() {
            return text.to_owned();
        }
        self.pending.extend_from_slice(text.as_bytes());
        // Only whole characters go in and come out, so it stays UTF-8.
        utf8(self.advance(true))
    }

    /// What can be written of the bytes so far, redacted.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> Vec<u8> {
        if self.redactor.automaton.is_none() {
            return bytes.to_vec();
        }
        self.pending.extend_from_slice(bytes);
        self.advance(false)
    }

    /// Takes off what can be shown: everything but an end that could still
    /// become a value, and never part of a value it found. `text`: cut only
    /// between characters.
    fn advance(&mut self, text: bool) -> Vec<u8> {
        let Some(automaton) = &self.redactor.automaton else { return std::mem::take(&mut self.pending) };
        let mut cut = self.redactor.undecided(&self.pending);
        while text && cut > 0 && cut < self.pending.len() && self.pending[cut] & 0xC0 == 0x80 {
            cut -= 1;
        }
        let mut out = Vec::new();
        let mut last = 0;
        for found in automaton.find_iter(self.pending.as_slice()) {
            if found.start() >= cut {
                break;
            }
            out.extend_from_slice(&self.pending[last..found.start()]);
            out.extend_from_slice(self.redactor.replacements[found.pattern().as_usize()].as_bytes());
            last = found.end();
            cut = cut.max(found.end());
        }
        out.extend_from_slice(&self.pending[last..cut]);
        self.pending.drain(..cut);
        out
    }

    /// The rest, at the end of the stream, redacted.
    pub fn finish(&mut self) -> String {
        utf8(self.finish_bytes())
    }

    /// The rest, at the end of a stream of bytes, redacted.
    pub fn finish_bytes(&mut self) -> Vec<u8> {
        let rest = std::mem::take(&mut self.pending);
        self.redactor.redact_bytes(&rest).into_owned()
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
