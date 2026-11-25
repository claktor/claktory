//! Report types (pipes-automation.md §2, §11): what a type is, and the check
//! every submitted report passes before it's kept: only the type's fields;
//! facts exactly one of their choices, or in their range (Claktory's copy
//! is kept, never the submitted string); text normalised, stripped of
//! characters that hide or reorder what's shown, and within its caps.
//! Refusals name a code and, at most, one of the type's own field names:
//! never anything submitted.
//!
//! The field kinds are a closed set, and whether a field is the other
//! side's words or a fact follows its kind, never a flag (Fugo's invariants
//! 1 and 2). Types are data, kept in factory.db (`types`); the built-in
//! ones are made here and kept there at version 1.

use std::sync::OnceLock;

use factory_agent::text::hidden;
use factory_protocol::v1;
use serde_json::{Map, Value, json};
use unicode_normalization::UnicodeNormalization;

/// The longest report, in bytes of text: a pipe message's cap.
pub(crate) const MAX_REPORT_BYTES: usize = 16 * 1024;

// The ceilings a type stays within (ReportTypeRules): a type can only set
// lower ones.
pub(crate) const TEXT_BYTES: usize = 4096;
pub(crate) const LIST_ITEMS: usize = 20;
pub(crate) const ITEM_BYTES: usize = 1024;
pub(crate) const FIELDS: usize = 20;
pub(crate) const FACTS: usize = 12;
pub(crate) const CHOICES: usize = 32;
pub(crate) const ATTACHMENTS: usize = 3;
pub(crate) const ATTACHMENT_MB: u32 = 2;
pub(crate) const NUMBER_ABS: i64 = 1_000_000_000;
/// A Number's range when its author gives none.
pub(crate) const NUMBER_DEFAULT: (i64, i64) = (0, 1_000_000);
/// Number and Date fields in a type: facts reach the reader without marking
/// it, so how much they can carry stays small (Fugo).
pub(crate) const NUMBERS: usize = 4;
pub(crate) const DATES: usize = 2;
pub(crate) const NAME_CHARS: usize = 60;
pub(crate) const LABEL_CHARS: usize = 80;
pub(crate) const RULE_CHARS: usize = 200;
/// Types in a Claktory (not deleted), and enabled on one pipe.
pub(crate) const TYPES: usize = 50;
pub(crate) const PER_PIPE: usize = 10;
/// The longest id, a type's, a field's or a choice's.
pub(crate) const ID_CHARS: usize = 40;
/// Field ids that would read as routing (Fugo's invariant 5).
pub(crate) const RESERVED: &[&str] = &["to", "recipient", "agent", "cc", "forward", "path", "url", "bcc", "reply_to", "send", "pipe", "project"];
/// Words that make any id read as routing, between its _s ("forward_note"),
/// or with an s ("urls"). Common words that only sometimes do (notify,
/// target, file_path, project_name) aren't, since ids never route anyway.
pub(crate) const ROUTING_TOKENS: &[&str] =
    &["recipient", "forward", "cc", "bcc", "url", "link", "agent", "pipe", "email", "webhook", "endpoint", "destination"];

/// A field id that reads as saying where the report goes: reserved, with a
/// routing word in it, or ending "_to" ("send_to"; "time_to_fix" passes).
pub(crate) fn routing(id: &str) -> bool {
    let word = |token: &str| ROUTING_TOKENS.contains(&token) || token.strip_suffix('s').is_some_and(|one| ROUTING_TOKENS.contains(&one));
    RESERVED.contains(&id) || id.ends_with("_to") || id.split('_').any(word)
}
/// A Date's range.
const FIRST_DAY: (i16, i8, i8) = (1900, 1, 1);
const LAST_DAY: (i16, i8, i8) = (2999, 12, 31);
/// Text fields this short are one line.
const ONE_LINE_BYTES: usize = 240;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Text,
    TextList,
    Enum(Vec<String>),
    Bool,
    /// A known release, or "unknown".
    Version,
    /// Files in the pipe's inbox, copied into the report (`attachments`).
    Attachments,
    /// A whole number in its range.
    Number { min: i64, max: i64 },
    /// A day, "2026-11-22".
    Date,
}

impl Kind {
    /// Its name, as kept and as Rivet's tool takes it.
    pub(crate) fn id(&self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::TextList => "text_list",
            Kind::Enum(_) => "enum",
            Kind::Bool => "bool",
            Kind::Version => "version",
            Kind::Attachments => "attachments",
            Kind::Number { .. } => "number",
            Kind::Date => "date",
        }
    }

    /// The other side's words: reading them marks the reader. A function of
    /// the kind alone (invariant 2).
    pub(crate) fn words(&self) -> bool {
        matches!(self, Kind::Text | Kind::TextList | Kind::Attachments)
    }

    fn proto(&self) -> v1::ReportFieldType {
        match self {
            Kind::Text => v1::ReportFieldType::Text,
            Kind::TextList => v1::ReportFieldType::TextList,
            Kind::Enum(_) => v1::ReportFieldType::Enum,
            Kind::Bool => v1::ReportFieldType::Bool,
            Kind::Version => v1::ReportFieldType::Version,
            Kind::Attachments => v1::ReportFieldType::Attachments,
            Kind::Number { .. } => v1::ReportFieldType::Number,
            Kind::Date => v1::ReportFieldType::Date,
        }
    }
}

/// The kinds whose values are words, for ReportTypeRules.
pub(crate) const WORD_KINDS: [v1::ReportFieldType; 3] = [v1::ReportFieldType::Text, v1::ReportFieldType::TextList, v1::ReportFieldType::Attachments];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Field {
    pub name: String,
    pub label: String,
    pub kind: Kind,
    pub required: bool,
    /// Text: the most bytes (of each item).
    pub max_bytes: usize,
    /// A list: the most items.
    pub max_items: usize,
    pub rule: String,
    /// The rule was written, not made by Claktory from the kind and caps.
    pub rule_written: bool,
}

impl Field {
    /// A fact: reading it affects no agent.
    pub(crate) fn fact(&self) -> bool {
        !self.kind.words()
    }

    /// Claktory's own rule for it.
    pub(crate) fn made_rule(&self) -> String {
        match &self.kind {
            Kind::Text if self.max_bytes <= ONE_LINE_BYTES => format!("One line, up to {} bytes", thousands(self.max_bytes as i64)),
            Kind::Text => format!("Up to {} bytes", thousands(self.max_bytes as i64)),
            Kind::TextList => format!("Up to {} items of {} bytes each", self.max_items, thousands(self.max_bytes as i64)),
            Kind::Enum(_) => "One of its choices".to_owned(),
            Kind::Bool => "Yes or no".to_owned(),
            Kind::Version => "A Claktory release, such as v0.1.16, or unknown".to_owned(),
            Kind::Attachments if self.required => format!("1 to {} files from the pipe's inbox", self.max_items),
            Kind::Attachments => format!("Up to {} files from the pipe's inbox", self.max_items),
            Kind::Number { min, max } => format!("A whole number from {} to {}", thousands(*min), thousands(*max)),
            Kind::Date => "A day, as YYYY-MM-DD".to_owned(),
        }
    }
}

/// "2,000", "-1,000,000,000".
pub(crate) fn thousands(number: i64) -> String {
    let digits = number.unsigned_abs().to_string();
    let mut shown = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            shown.push(',');
        }
        shown.push(digit);
    }
    if number < 0 { format!("-{shown}") } else { shown }
}

/// What a reader can tell the other side of a report (handoff_mark): a
/// closed catalogue, its words Claktory's; a type chooses which.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Outcome {
    Accepted,
    Done,
    Declined,
    Known,
    Fixed,
    WontFix,
    NeedsInfo,
}

impl Outcome {
    pub(crate) const ALL: [Outcome; 7] =
        [Outcome::Accepted, Outcome::Done, Outcome::Declined, Outcome::Known, Outcome::Fixed, Outcome::WontFix, Outcome::NeedsInfo];

    /// As kept (a report's state) and as handoff_mark takes it.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Outcome::Accepted => "accepted",
            Outcome::Done => "done",
            Outcome::Declined => "declined",
            Outcome::Known => "known",
            Outcome::Fixed => "fixed",
            Outcome::WontFix => "wont_fix",
            Outcome::NeedsInfo => "needs_info",
        }
    }

    pub(crate) fn of(id: &str) -> Option<Outcome> {
        Outcome::ALL.into_iter().find(|outcome| outcome.id() == id)
    }

    pub(crate) fn proto(self) -> v1::PipeReportState {
        match self {
            Outcome::Accepted => v1::PipeReportState::Accepted,
            Outcome::Done => v1::PipeReportState::Done,
            Outcome::Declined => v1::PipeReportState::Declined,
            Outcome::Known => v1::PipeReportState::Known,
            Outcome::Fixed => v1::PipeReportState::Fixed,
            Outcome::WontFix => v1::PipeReportState::WontFix,
            Outcome::NeedsInfo => v1::PipeReportState::NeedsInfo,
        }
    }

    pub(crate) fn from_proto(state: i32) -> Option<Outcome> {
        Outcome::ALL.into_iter().find(|outcome| outcome.proto() as i32 == state)
    }
}

/// One version of a type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Schema {
    pub id: String,
    pub version: u32,
    pub name: String,
    pub fields: Vec<Field>,
    pub outcomes: Vec<Outcome>,
    pub builtin: bool,
    /// "" for the person (or Claktory, for a built-in's first), else the
    /// admin agent's id.
    pub by: String,
    pub at: i64,
    /// Every version of a deleted type is.
    pub deleted: bool,
    /// The fields it added or widened over the version before.
    pub widened: Vec<String>,
}

impl Schema {
    pub(crate) fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }

    /// Whether it lets more of the other side's words through than the
    /// version before: a widened field that carries words. More choices, a
    /// wider range or another fact widen what crosses (`widened`), but no
    /// words of theirs.
    pub(crate) fn widens_words(&self) -> bool {
        self.widened.iter().any(|name| self.field(name).is_some_and(|field| field.kind.words()))
    }

    /// Its files' field, if it has one (at most one).
    pub(crate) fn attachments(&self) -> Option<&Field> {
        self.fields.iter().find(|field| field.kind == Kind::Attachments)
    }

    pub(crate) fn allows(&self, outcome: Outcome) -> bool {
        self.outcomes.contains(&outcome)
    }
}

pub(crate) const BUG_REPORT: &str = "bug_report";

const PLATFORMS: &[&str] = &["mac", "iphone", "ipad", "android", "linux_server", "web", "unknown"];
const AREAS: &[&str] = &["agents", "pipes", "people", "devices", "connection", "settings", "map", "usage", "updates", "install", "other"];
const SEVERITIES: &[&str] = &["crash", "data_loss", "broken", "annoying", "cosmetic"];
const REPRODUCIBLE: &[&str] = &["always", "sometimes", "once", "unknown"];

fn field(name: &str, label: &str, kind: Kind, required: bool, max_bytes: usize, max_items: usize, rule: &str) -> Field {
    let mut field = Field {
        name: name.to_owned(),
        label: label.to_owned(),
        kind,
        required,
        max_bytes,
        max_items,
        rule: rule.to_owned(),
        rule_written: !rule.is_empty(),
    };
    if field.rule.is_empty() {
        field.rule = field.made_rule();
    }
    field
}

fn text(name: &str, label: &str, required: bool, max_bytes: usize) -> Field {
    field(name, label, Kind::Text, required, max_bytes, 0, "")
}

fn choice(name: &str, label: &str, choices: &[&str]) -> Field {
    field(name, label, Kind::Enum(choices.iter().map(|choice| (*choice).to_owned()).collect()), true, 0, 0, "")
}

fn files(required: bool) -> Field {
    field("attachments", "Files", Kind::Attachments, required, 0, 3, "")
}

fn builtin(id: &str, name: &str, fields: Vec<Field>, outcomes: &[Outcome]) -> Schema {
    Schema {
        id: id.to_owned(),
        version: 1,
        name: name.to_owned(),
        fields,
        outcomes: outcomes.to_vec(),
        builtin: true,
        by: String::new(),
        at: 0,
        deleted: false,
        widened: Vec::new(),
    }
}

/// The built-in types, at version 1: "Bug report" and Fugo's five starters
/// (§11.5), in the order they're listed.
pub(crate) fn builtins() -> Vec<Schema> {
    use Outcome::*;
    vec![
        builtin(
            BUG_REPORT,
            "Bug report",
            vec![
                field("title", "Title", Kind::Text, true, 240, 0, "One line, up to 240 bytes"),
                field("what_happened", "What happened", Kind::Text, true, 2000, 0, "Up to 2,000 bytes"),
                field("expected", "What they expected", Kind::Text, false, 1000, 0, "Up to 1,000 bytes"),
                field("steps", "Steps", Kind::TextList, false, 1024, 10, "Up to 10 steps of 1,024 bytes each"),
                field("platform", "Platform", Kind::Enum(PLATFORMS.iter().map(|choice| (*choice).to_owned()).collect()), true, 0, 0, "One of its choices"),
                field("app_version", "Version", Kind::Version, true, 0, 0, "A Claktory release, such as v0.1.16, or unknown"),
                field("area", "Area", Kind::Enum(AREAS.iter().map(|choice| (*choice).to_owned()).collect()), true, 0, 0, "One of its choices"),
                field("severity", "Severity", Kind::Enum(SEVERITIES.iter().map(|choice| (*choice).to_owned()).collect()), true, 0, 0, "One of its choices"),
                field("reproducible", "Happens", Kind::Enum(REPRODUCIBLE.iter().map(|choice| (*choice).to_owned()).collect()), true, 0, 0, "One of its choices"),
                field("friend_confirmed", "Confirmed by the other side", Kind::Bool, true, 0, 0, "Yes or no: the other side confirmed this summary"),
                field(
                    "attachments",
                    "Files",
                    Kind::Attachments,
                    false,
                    0,
                    3,
                    "Up to 3 files from the pipe's inbox (screenshots, logs), 2 MB each",
                ),
            ],
            &[Accepted, Known, Fixed, WontFix, NeedsInfo],
        ),
        builtin(
            "task_request",
            "Task request",
            vec![
                text("title", "Title", true, 240),
                text("needed", "What's needed", true, 2000),
                text("done_when", "Done when", false, 1000),
                files(false),
                choice("priority", "Priority", &["low", "normal", "high", "urgent"]),
                field("due", "Due", Kind::Date, false, 0, 0, ""),
            ],
            &[Accepted, Done, Declined, Known, NeedsInfo],
        ),
        builtin(
            "question",
            "Question",
            vec![text("question", "Question", true, 1000), text("context", "Context", false, 2000), choice("urgency", "Urgency", &["whenever", "today", "now"])],
            &[Accepted, Done, Declined, Known, NeedsInfo],
        ),
        builtin(
            "file_share",
            "File share",
            vec![
                text("title", "Title", true, 240),
                text("description", "Description", false, 2000),
                files(true),
                choice("kind", "Type", &["dataset", "document", "image", "log", "other"]),
            ],
            &[Accepted, Done, Declined, NeedsInfo],
        ),
        builtin(
            "status_update",
            "Status update",
            vec![
                text("subject", "Subject", true, 240),
                text("summary", "Summary", true, 2000),
                field("blockers", "Blockers", Kind::TextList, false, 512, 5, ""),
                choice("state", "State", &["on_track", "at_risk", "blocked", "done"]),
            ],
            &[Accepted, NeedsInfo],
        ),
        builtin(
            "feature_idea",
            "Feature idea",
            vec![
                text("title", "Title", true, 240),
                text("problem", "Problem", true, 2000),
                text("proposal", "Proposal", false, 2000),
                files(false),
                choice("area", "Area", AREAS),
            ],
            &[Accepted, Done, Declined, Known, NeedsInfo],
        ),
    ]
}

/// The releases a version field accepts: every one in this build's
/// changelog, and this build's own.
pub(crate) fn known_versions() -> &'static [String] {
    static KNOWN: OnceLock<Vec<String>> = OnceLock::new();
    KNOWN.get_or_init(|| {
        let mut versions: Vec<String> = crate::update::parse_changelog(include_str!("../../../../../docs/CHANGELOG.md"))
            .into_iter()
            .map(|version| version.version)
            .filter(|version| !version.is_empty())
            .collect();
        if crate::update::plain(crate::update::VERSION) && !versions.iter().any(|version| version == crate::update::VERSION) {
            versions.push(crate::update::VERSION.to_owned());
        }
        versions
    })
}

/// Claktory's own copy of `version`, when it's a known release or "unknown".
pub(crate) fn known_version(version: &str) -> Option<&'static str> {
    if version == "unknown" {
        return Some("unknown");
    }
    known_versions().iter().find(|known| known.as_str() == version).map(String::as_str)
}

/// Claktory's own copy of a day, "2026-11-22", when it's a real one in range.
pub(crate) fn known_day(day: &str) -> Option<String> {
    let bytes = day.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes.iter().enumerate().all(|(index, byte)| index == 4 || index == 7 || byte.is_ascii_digit());
    if !shaped {
        return None;
    }
    let date = jiff::civil::Date::new(day[..4].parse().ok()?, day[5..7].parse().ok()?, day[8..].parse().ok()?).ok()?;
    let first = jiff::civil::Date::new(FIRST_DAY.0, FIRST_DAY.1, FIRST_DAY.2).ok()?;
    let last = jiff::civil::Date::new(LAST_DAY.0, LAST_DAY.1, LAST_DAY.2).ok()?;
    (first <= date && date <= last).then(|| date.strftime("%Y-%m-%d").to_string())
}

/// Why a report wasn't taken: a fixed code, and one of the type's own
/// field names when it's about one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub code: &'static str,
    pub field: Option<String>,
}

impl Refusal {
    pub(crate) fn new(code: &'static str) -> Refusal {
        Refusal { code, field: None }
    }

    pub(crate) fn of(code: &'static str, field: &Field) -> Refusal {
        Refusal { code, field: Some(field.name.clone()) }
    }

    /// What the liaison is told: "refused: too_long what_happened".
    pub(crate) fn said(&self) -> String {
        match &self.field {
            Some(field) => format!("refused: {} {field}", self.code),
            None => format!("refused: {}", self.code),
        }
    }
}

/// A report as checked: each field Claktory's own value, and its text's
/// size in bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Checked {
    pub fields: Map<String, Value>,
    pub text_bytes: usize,
}

impl Checked {
    /// Its words, for the secret scan.
    pub(crate) fn text(&self, schema: &Schema) -> String {
        let mut text = String::new();
        for field in schema.fields.iter().filter(|field| !field.fact() && field.kind != Kind::Attachments) {
            match self.fields.get(&field.name) {
                Some(Value::String(value)) => {
                    text.push_str(value);
                    text.push('\n');
                }
                Some(Value::Array(items)) => {
                    for item in items.iter().filter_map(Value::as_str) {
                        text.push_str(item);
                        text.push('\n');
                    }
                }
                _ => {}
            }
        }
        text
    }
}

/// Checks `submitted` (but for `ignore`d keys) against `schema`.
pub(crate) fn check(schema: &Schema, submitted: &Map<String, Value>, ignore: &[&str]) -> Result<Checked, Refusal> {
    let mut checked = Checked::default();
    for (key, _) in submitted.iter().filter(|(key, _)| !ignore.contains(&key.as_str())) {
        if schema.field(key).is_none() {
            return Err(Refusal::new("unknown_field"));
        }
    }
    for field in &schema.fields {
        let value = submitted.get(&field.name).filter(|value| !value.is_null());
        let invalid = || Refusal::of("invalid", field);
        let kept = match (&field.kind, value) {
            (_, None) => None,
            (Kind::Text, Some(value)) => {
                let Value::String(text) = value else { return Err(invalid()) };
                let text = cleaned(text, field.max_bytes <= ONE_LINE_BYTES);
                if text.len() > field.max_bytes {
                    return Err(Refusal::of("too_long", field));
                }
                checked.text_bytes += text.len();
                (!text.is_empty()).then_some(Value::String(text))
            }
            (Kind::TextList, Some(value)) => {
                let Value::Array(items) = value else { return Err(invalid()) };
                if items.len() > field.max_items {
                    return Err(Refusal::of("too_many", field));
                }
                let mut kept = Vec::new();
                for item in items {
                    let Value::String(item) = item else { return Err(invalid()) };
                    let item = cleaned(item, false);
                    if item.len() > field.max_bytes {
                        return Err(Refusal::of("too_long", field));
                    }
                    checked.text_bytes += item.len();
                    if !item.is_empty() {
                        kept.push(Value::String(item));
                    }
                }
                (!kept.is_empty()).then_some(Value::Array(kept))
            }
            (Kind::Enum(choices), Some(value)) => {
                let choice = value.as_str().and_then(|value| choices.iter().find(|choice| choice.as_str() == value));
                let Some(choice) = choice else { return Err(invalid()) };
                Some(Value::String(choice.clone()))
            }
            (Kind::Bool, Some(value)) => {
                let Value::Bool(flag) = value else { return Err(invalid()) };
                Some(Value::Bool(*flag))
            }
            (Kind::Version, Some(value)) => {
                let Some(version) = value.as_str().and_then(known_version) else { return Err(Refusal::of("version", field)) };
                Some(Value::String(version.to_owned()))
            }
            (Kind::Number { min, max }, Some(value)) => {
                let number = value.as_i64().or_else(|| value.as_f64().filter(|number| number.fract() == 0.0 && number.abs() <= 1e15).map(|number| number as i64));
                let Some(number) = number else { return Err(invalid()) };
                if number < *min || number > *max {
                    return Err(Refusal::of("out_of_range", field));
                }
                Some(Value::from(number))
            }
            (Kind::Date, Some(value)) => {
                let Some(day) = value.as_str().and_then(known_day) else { return Err(Refusal::of("date", field)) };
                Some(Value::String(day))
            }
            // Handles, checked and copied by the caller (`attachments::take`).
            (Kind::Attachments, Some(value)) => {
                let Value::Array(items) = value else { return Err(invalid()) };
                if items.len() > field.max_items {
                    return Err(Refusal::of("too_many", field));
                }
                if items.iter().any(|item| !item.as_str().is_some_and(|item| !item.trim().is_empty() && item.len() <= 1024)) {
                    return Err(invalid());
                }
                (!items.is_empty()).then(|| value.clone())
            }
        };
        match kept {
            Some(kept) => {
                checked.fields.insert(field.name.clone(), kept);
            }
            None if field.required => return Err(Refusal::of("missing", field)),
            None => {}
        }
    }
    if checked.text_bytes > MAX_REPORT_BYTES {
        return Err(Refusal::new("too_long"));
    }
    Ok(checked)
}

/// Text as kept: NFC, without control characters (but line breaks and
/// tabs, unless `one_line`) or the ones that hide or reorder what's shown
/// (bidi controls, zero-width and tag characters), trimmed.
pub(crate) fn cleaned(text: &str, one_line: bool) -> String {
    let text = text.replace("\r\n", "\n");
    let mut kept = String::with_capacity(text.len());
    for character in text.nfc() {
        let keep = match character {
            '\n' | '\t' => !one_line,
            _ if character.is_control() => false,
            _ => !hidden(character),
        };
        if keep {
            kept.push(character);
        } else if one_line && matches!(character, '\n' | '\t') {
            kept.push(' ');
        }
    }
    kept.trim().to_owned()
}

/// A type's fields as a tool's parameters: typed, and `extra`. Only types:
/// what's required, ranges, how many items and unknown fields are checked
/// here (`check`), so a refusal never echoes what was sent, as the agent's
/// own validation of arguments would. Its words (name, labels, rules) are
/// the author's, escaped: shown, never obeyed.
pub(crate) fn parameters(schema: &Schema, extra: Map<String, Value>) -> Value {
    use crate::pipes::agents::escape;
    let mut properties = Map::new();
    let mut required = Vec::new();
    for field in &schema.fields {
        let description = format!("{}: {}.", escape(&field.label), escape(&field.rule));
        let property = match &field.kind {
            Kind::Text | Kind::Version | Kind::Date => json!({"type": "string", "description": description}),
            Kind::TextList | Kind::Attachments => json!({"type": "array", "items": {"type": "string"}, "description": description}),
            Kind::Enum(choices) => json!({"type": "string", "enum": choices, "description": description}),
            Kind::Bool => json!({"type": "boolean", "description": description}),
            Kind::Number { .. } => json!({"type": "integer", "description": description}),
        };
        properties.insert(field.name.clone(), property);
        if field.required {
            required.push(field.name.as_str());
        }
    }
    properties.extend(extra);
    let required = if required.is_empty() { "none".to_owned() } else { required.join(", ") };
    json!({"type": "object", "description": format!("{} (\"{}\"). Required: {required}.", escape(&schema.name), schema.id), "properties": properties})
}

pub(crate) fn to_proto(schema: &Schema) -> v1::ReportSchema {
    v1::ReportSchema {
        id: schema.id.clone(),
        name: schema.name.clone(),
        fields: schema
            .fields
            .iter()
            .map(|field| v1::ReportField {
                name: field.name.clone(),
                label: field.label.clone(),
                fact: field.fact(),
                r#type: field.kind.proto() as i32,
                choices: match &field.kind {
                    Kind::Enum(choices) => choices.clone(),
                    _ => Vec::new(),
                },
                required: field.required,
                max_bytes: u32::try_from(field.max_bytes).unwrap_or(u32::MAX),
                max_items: u32::try_from(field.max_items).unwrap_or(u32::MAX),
                rule: field.rule.clone(),
                min: match field.kind {
                    Kind::Number { min, .. } => Some(min),
                    _ => None,
                },
                max: match field.kind {
                    Kind::Number { max, .. } => Some(max),
                    _ => None,
                },
                rule_written: field.rule_written,
            })
            .collect(),
        version: schema.version,
        builtin: schema.builtin,
        by_id: schema.by.clone(),
        at_ms: schema.at,
        outcomes: schema.outcomes.iter().map(|outcome| outcome.proto() as i32).collect(),
        widened_fields: schema.widened.clone(),
    }
}

/// Workspace.report_type_rules.
pub(crate) fn rules_proto() -> v1::ReportTypeRules {
    let count = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    v1::ReportTypeRules {
        text_bytes: count(TEXT_BYTES),
        list_items: count(LIST_ITEMS),
        item_bytes: count(ITEM_BYTES),
        fields: count(FIELDS),
        facts: count(FACTS),
        choices: count(CHOICES),
        attachments: count(ATTACHMENTS),
        attachment_mb: ATTACHMENT_MB,
        number_abs: NUMBER_ABS,
        name_chars: count(NAME_CHARS),
        label_chars: count(LABEL_CHARS),
        rule_chars: count(RULE_CHARS),
        types: count(TYPES),
        per_pipe: count(PER_PIPE),
        numbers: count(NUMBERS),
        dates: count(DATES),
        reserved_ids: RESERVED.iter().map(|id| (*id).to_owned()).collect(),
        routing_tokens: ROUTING_TOKENS.iter().map(|token| (*token).to_owned()).collect(),
        text_kinds: WORD_KINDS.iter().map(|kind| *kind as i32).collect(),
    }
}

/// A field's value as the app shows it.
pub(crate) fn value_proto(field: &str, value: &Value) -> v1::ReportValue {
    let mut shown = v1::ReportValue { field: field.to_owned(), ..Default::default() };
    match value {
        Value::String(text) => shown.text = text.clone(),
        Value::Array(items) => shown.items = items.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
        Value::Bool(flag) => shown.flag = *flag,
        Value::Number(number) => shown.number = number.as_i64().unwrap_or_default(),
        _ => {}
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bug_report() -> Schema {
        builtins().into_iter().next().expect("Bug report")
    }

    fn bug(extra: Value) -> Map<String, Value> {
        let mut report = json!({
            "title": "Crash on open",
            "what_happened": "It quit.",
            "platform": "iphone",
            "app_version": "unknown",
            "area": "agents",
            "severity": "crash",
            "reproducible": "always",
            "friend_confirmed": true,
        });
        for (key, value) in extra.as_object().cloned().unwrap_or_default() {
            report[key] = value;
        }
        report.as_object().cloned().unwrap_or_default()
    }

    fn checked(extra: Value) -> Result<Checked, Refusal> {
        check(&bug_report(), &bug(extra), &["token"])
    }

    fn refusal(code: &'static str, field: &str) -> Refusal {
        Refusal { code, field: Some(field.to_owned()) }
    }

    #[test]
    fn a_report_keeps_claktorys_copy_of_each_fact_and_clean_text() {
        let report = checked(json!({"title": "Crash\u{202E}gnp.exe\u{200B} on\nopen", "steps": ["a", "  ", "b\u{E0041}"], "token": "x"})).expect("taken");
        assert_eq!(report.fields["title"], "Crashgnp.exe on open");
        assert_eq!(report.fields["steps"], json!(["a", "b"]));
        assert_eq!(report.fields["platform"], "iphone");
        assert!(!report.fields.contains_key("token"));
        assert_eq!(report.text_bytes, "Crashgnp.exe on open".len() + "It quit.".len() + 2);
    }

    #[test]
    fn a_refusal_names_a_code_and_only_the_schemas_own_fields() {
        let marker = "MARKER-7f3a";
        let refused = |extra: Value| checked(extra).expect_err("refused");
        assert_eq!(refused(json!({marker: "x"})), Refusal::new("unknown_field"));
        assert_eq!(refused(json!({"platform": "iphone "})), refusal("invalid", "platform"));
        // A Cyrillic "а" that looks like the Latin one.
        assert_eq!(refused(json!({"area": "\u{0430}gents"})), refusal("invalid", "area"));
        assert_eq!(refused(json!({"friend_confirmed": "true"})), refusal("invalid", "friend_confirmed"));
        assert_eq!(refused(json!({"app_version": format!("v0.1.16 {marker}")})), refusal("version", "app_version"));
        assert_eq!(refused(json!({"what_happened": "\u{200B} "})), refusal("missing", "what_happened"));
        assert_eq!(refused(json!({"title": "x".repeat(241)})), refusal("too_long", "title"));
        assert_eq!(refused(json!({"steps": vec!["x"; 11]})), refusal("too_many", "steps"));
        for refusal in [refused(json!({marker: 1})), refused(json!({"severity": marker}))] {
            assert!(!refusal.said().contains(marker), "{}", refusal.said());
        }
    }

    #[test]
    fn the_whole_report_has_a_cap() {
        let steps = vec!["x".repeat(1024); 10];
        let refusal = checked(json!({"what_happened": "y".repeat(2000), "expected": "z".repeat(1000), "steps": steps, "title": "t".repeat(240)}));
        assert!(refusal.is_ok(), "13,464 bytes fit");
        let mut big = bug_report();
        for field in big.fields.iter_mut().filter(|field| field.kind == Kind::Text) {
            field.max_bytes = TEXT_BYTES;
        }
        let report = bug(json!({"what_happened": "y".repeat(4096), "expected": "z".repeat(4096), "title": "t".repeat(4096), "steps": vec!["x".repeat(1024); 10]}));
        assert_eq!(check(&big, &report, &[]), Err(Refusal::new("too_long")));
    }

    #[test]
    fn numbers_and_days_are_range_checked_and_kept_as_claktorys_own() {
        let mut schema = bug_report();
        schema.fields = vec![
            field("count", "Count", Kind::Number { min: -5, max: 10 }, true, 0, 0, ""),
            field("due", "Due", Kind::Date, false, 0, 0, ""),
        ];
        let try_one = |value: Value| check(&schema, value.as_object().expect("object"), &[]);
        assert_eq!(try_one(json!({"count": 10, "due": "2026-11-22"})).expect("taken").fields, json!({"count": 10, "due": "2026-11-22"}).as_object().cloned().expect("object"));
        assert_eq!(try_one(json!({"count": 3.0})).expect("taken").fields["count"], 3);
        assert_eq!(try_one(json!({"count": 11})), Err(refusal("out_of_range", "count")));
        assert_eq!(try_one(json!({"count": -6})), Err(refusal("out_of_range", "count")));
        assert_eq!(try_one(json!({"count": 1.5})), Err(refusal("invalid", "count")));
        assert_eq!(try_one(json!({"count": "3"})), Err(refusal("invalid", "count")));
        for day in ["2026-02-30", "2026-1-22", "1899-12-31", "3000-01-01", "２026-11-22", "2026-11-22 ", "2026-11-22T00:00"] {
            assert_eq!(try_one(json!({"count": 1, "due": day})), Err(refusal("date", "due")), "{day}");
        }
        assert_eq!(known_day("2999-12-31").as_deref(), Some("2999-12-31"));
        assert_eq!(known_day("1900-01-01").as_deref(), Some("1900-01-01"));
    }

    #[test]
    fn whether_a_field_is_words_follows_its_kind() {
        for schema in builtins() {
            for field in &schema.fields {
                assert_eq!(field.fact(), !matches!(field.kind, Kind::Text | Kind::TextList | Kind::Attachments), "{}", field.name);
            }
        }
    }

    #[test]
    fn versions_are_this_builds_releases() {
        assert_eq!(known_version("unknown"), Some("unknown"));
        assert!(known_versions().iter().any(|version| version == "v0.1.16"), "{:?}", known_versions());
        assert_eq!(known_version("v0.1.16"), Some("v0.1.16"));
        assert_eq!(known_version("V0.1.16"), None);
        assert_eq!(known_version("v9.9.9"), None);
    }

    #[test]
    fn thousands_reads_as_written() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(2000), "2,000");
        assert_eq!(thousands(-1_000_000_000), "-1,000,000,000");
    }
}
