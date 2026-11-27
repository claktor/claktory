//! Report types as data: anyone untainted defines one (the person in the
//! app, an admin agent through pipes_report_type while it has read nothing
//! from outside), and a pipe enables a few of them. A type can only narrow
//! what crosses from a liaison to its reader: its fields are of the closed
//! kinds (`schema::Kind`), within the ceilings in code, and no field
//! addresses anyone (each rule checked in `validate`).
//!
//! Kept in factory.db, `pipe_handoff_types`, keyed on (id, version): a
//! version never changes, an edit makes the next, and a deleted type keeps
//! its versions for the reports that use them. The built-in types are kept
//! at version 1 the first time they're needed. Every definition, edit and
//! deletion is kept in `pipe_handoff_changes` for Needs you's history.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, PoisonError};

use factory_protocol::v1;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use super::schema::{self, Field, Kind, Outcome, Schema};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Report types, each version as it was made: never changed, only added to.
CREATE TABLE pipe_handoff_types (
    id TEXT NOT NULL,
    version INTEGER NOT NULL,
    name TEXT NOT NULL,
    -- JSON: the fields (handoff::types::Stored).
    fields TEXT NOT NULL,
    -- JSON: the outcomes its reader may use ('accepted', 'done'…).
    outcomes TEXT NOT NULL,
    builtin INTEGER NOT NULL,
    -- '' for the person (or Claktory, a built-in's first), else the admin agent's id.
    by TEXT NOT NULL,
    at INTEGER NOT NULL,
    -- Every version of a deleted type is.
    deleted INTEGER NOT NULL DEFAULT 0,
    -- JSON: the fields it added or widened over the version before.
    widened TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (id, version)
);
-- JSON: the types a forward lets its liaison forward; '' reads as [schema].
ALTER TABLE pipe_handoff_forwards ADD COLUMN types TEXT NOT NULL DEFAULT '';
UPDATE pipe_handoff_forwards SET types = '[\"' || schema || '\"]';
-- The version of its type (schema) a report was checked against.
ALTER TABLE pipe_handoff_reports ADD COLUMN type_version INTEGER NOT NULL DEFAULT 1;
";

/// Every version of every type, as read from factory.db; dropped on each
/// change.
#[derive(Default)]
pub(crate) struct Cache(Mutex<Option<Arc<Vec<Arc<Schema>>>>>);

/// A field as kept.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Stored {
    name: String,
    label: String,
    kind: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    choices: Vec<String>,
    required: bool,
    max_bytes: usize,
    max_items: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<i64>,
    rule: String,
    rule_written: bool,
}

impl Stored {
    fn of(field: &Field) -> Stored {
        let (choices, min, max) = match &field.kind {
            Kind::Enum(choices) => (choices.clone(), None, None),
            Kind::Number { min, max } => (Vec::new(), Some(*min), Some(*max)),
            _ => (Vec::new(), None, None),
        };
        Stored {
            name: field.name.clone(),
            label: field.label.clone(),
            kind: field.kind.id().to_owned(),
            choices,
            required: field.required,
            max_bytes: field.max_bytes,
            max_items: field.max_items,
            min,
            max,
            rule: field.rule.clone(),
            rule_written: field.rule_written,
        }
    }

    /// None for a kind this build doesn't know (kept by a newer one).
    fn field(self) -> Option<Field> {
        let kind = kind(&self.kind, self.choices, self.min, self.max)?;
        Some(Field {
            name: self.name,
            label: self.label,
            kind,
            required: self.required,
            max_bytes: self.max_bytes,
            max_items: self.max_items,
            rule: self.rule,
            rule_written: self.rule_written,
        })
    }
}

fn kind(id: &str, choices: Vec<String>, min: Option<i64>, max: Option<i64>) -> Option<Kind> {
    Some(match id {
        "text" => Kind::Text,
        "text_list" => Kind::TextList,
        "enum" => Kind::Enum(choices),
        "bool" => Kind::Bool,
        "version" => Kind::Version,
        "attachments" => Kind::Attachments,
        "number" => Kind::Number { min: min.unwrap_or(schema::NUMBER_DEFAULT.0), max: max.unwrap_or(schema::NUMBER_DEFAULT.1) },
        "date" => Kind::Date,
        _ => return None,
    })
}

const COLUMNS: &str = "id, version, name, fields, outcomes, builtin, by, at, deleted, widened";

fn schema_of(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Schema>> {
    let fields: Vec<Stored> = serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default();
    let fields: Option<Vec<Field>> = fields.into_iter().map(Stored::field).collect();
    let outcomes: Vec<String> = serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default();
    let Some(fields) = fields else { return Ok(None) };
    Ok(Some(Schema {
        id: row.get(0)?,
        version: row.get(1)?,
        name: row.get(2)?,
        fields,
        outcomes: outcomes.iter().filter_map(|outcome| Outcome::of(outcome)).collect(),
        builtin: row.get(5)?,
        by: row.get(6)?,
        at: row.get(7)?,
        deleted: row.get(8)?,
        widened: serde_json::from_str(&row.get::<_, String>(9)?).unwrap_or_default(),
    }))
}

fn insert(db: &Connection, schema: &Schema) -> rusqlite::Result<usize> {
    let fields: Vec<Stored> = schema.fields.iter().map(Stored::of).collect();
    let outcomes: Vec<&str> = schema.outcomes.iter().map(|outcome| outcome.id()).collect();
    db.execute(
        "INSERT OR IGNORE INTO pipe_handoff_types (id, version, name, fields, outcomes, builtin, by, at, deleted, widened)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
        params![
            schema.id,
            schema.version,
            schema.name,
            serde_json::to_string(&fields).unwrap_or_default(),
            serde_json::to_string(&outcomes).unwrap_or_default(),
            schema.builtin,
            schema.by,
            schema.at,
            serde_json::to_string(&schema.widened).unwrap_or_default(),
        ],
    )
}

/// Every version kept, the built-in types first kept at version 1.
fn load(db: &Connection) -> Vec<Arc<Schema>> {
    for builtin in schema::builtins() {
        if let Err(error) = insert(db, &builtin) {
            tracing::error!(id = %builtin.id, %error, "could not keep a built-in report type");
        }
    }
    let result = db.prepare(&format!("SELECT {COLUMNS} FROM pipe_handoff_types ORDER BY id, version")).and_then(|mut statement| {
        statement.query_map([], schema_of)?.collect::<rusqlite::Result<Vec<_>>>()
    });
    match result {
        Ok(rows) => rows.into_iter().flatten().map(Arc::new).collect(),
        Err(error) => {
            tracing::error!(%error, "could not read report types");
            // Still the built-in ones, so reports keep working.
            schema::builtins().into_iter().map(Arc::new).collect()
        }
    }
}

/// One version as kept, read from `db` itself (Needs you's history, which
/// holds it).
pub(crate) fn load_version(db: &Connection, id: &str, version: u32) -> Option<Schema> {
    db.query_row(&format!("SELECT {COLUMNS} FROM pipe_handoff_types WHERE id = ?1 AND version = ?2"), params![id, version], schema_of)
        .optional()
        .ok()
        .flatten()
        .flatten()
        .or_else(|| schema::builtins().into_iter().find(|builtin| builtin.id == id && version == 1))
}

fn all(factory: &Factory) -> Arc<Vec<Arc<Schema>>> {
    let mut cache = factory.pipes.handoff.types.0.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(all) = cache.as_ref() {
        return all.clone();
    }
    let all = Arc::new(load(&factory.store.db()));
    *cache = Some(all.clone());
    all
}

/// After a change: read again on next use. Never while holding the store.
fn forget(factory: &Factory) {
    *factory.pipes.handoff.types.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

/// The current version of `id`: its latest, unless it's deleted.
pub(crate) fn current(factory: &Factory, id: &str) -> Option<Arc<Schema>> {
    all(factory).iter().filter(|schema| schema.id == id && !schema.deleted).max_by_key(|schema| schema.version).cloned()
}

/// Any version, a deleted type's included.
pub(crate) fn version(factory: &Factory, id: &str, version: u32) -> Option<Arc<Schema>> {
    all(factory).iter().find(|schema| schema.id == id && schema.version == version).cloned()
}

/// The current types: built-in ones first, in their order, then by name.
pub(crate) fn currents(factory: &Factory) -> Vec<Arc<Schema>> {
    let all = all(factory);
    let order: Vec<String> = schema::builtins().into_iter().map(|builtin| builtin.id).collect();
    let ids: BTreeSet<&str> = all.iter().filter(|schema| !schema.deleted).map(|schema| schema.id.as_str()).collect();
    let mut currents: Vec<Arc<Schema>> = ids.into_iter().filter_map(|id| current(factory, id)).collect();
    currents.sort_by_key(|schema| (order.iter().position(|id| *id == schema.id).unwrap_or(usize::MAX), schema.name.to_lowercase()));
    currents
}

// ---- Defining a type ----

/// A definition as asked for, before any rule is checked.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Draft {
    pub id: String,
    pub name: String,
    pub fields: Vec<DraftField>,
    pub outcomes: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct DraftField {
    /// Its id; "" for a new field, made from its label.
    #[serde(alias = "id")]
    pub name: String,
    pub label: String,
    pub kind: String,
    pub choices: Vec<String>,
    pub required: bool,
    /// 0 for the ceiling.
    pub max_bytes: u64,
    pub max_items: u64,
    pub min: Option<i64>,
    pub max: Option<i64>,
    /// "" for Claktory's own.
    pub rule: String,
}

impl Draft {
    pub(crate) fn from_proto(schema: &v1::ReportSchema) -> Draft {
        Draft {
            id: schema.id.trim().to_owned(),
            name: schema.name.clone(),
            fields: schema
                .fields
                .iter()
                .map(|field| DraftField {
                    name: field.name.trim().to_owned(),
                    label: field.label.clone(),
                    kind: kind_id(field.r#type).to_owned(),
                    choices: field.choices.clone(),
                    required: field.required,
                    max_bytes: u64::from(field.max_bytes),
                    max_items: u64::from(field.max_items),
                    min: field.min,
                    max: field.max,
                    rule: field.rule.clone(),
                })
                .collect(),
            outcomes: schema.outcomes.iter().map(|state| Outcome::from_proto(*state).map_or("?", Outcome::id).to_owned()).collect(),
        }
    }
}

fn kind_id(kind: i32) -> &'static str {
    match v1::ReportFieldType::try_from(kind) {
        Ok(v1::ReportFieldType::Text) => "text",
        Ok(v1::ReportFieldType::TextList) => "text_list",
        Ok(v1::ReportFieldType::Enum) => "enum",
        Ok(v1::ReportFieldType::Bool) => "bool",
        Ok(v1::ReportFieldType::Version) => "version",
        Ok(v1::ReportFieldType::Attachments) => "attachments",
        Ok(v1::ReportFieldType::Number) => "number",
        Ok(v1::ReportFieldType::Date) => "date",
        _ => "",
    }
}

/// An id: lowercase letters, digits and _, at most 40.
pub(crate) fn is_id(text: &str) -> bool {
    !text.is_empty() && text.len() <= schema::ID_CHARS && text.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

/// An id made from `text` ("Frågor 2" → "fragor_2"), not one of `taken`.
fn minted(text: &str, fallback: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let mut base = String::new();
    for character in text.nfd() {
        if character.is_ascii_alphanumeric() {
            base.push(character.to_ascii_lowercase());
        } else if !matches!(u32::from(character), 0x300..=0x36F) && !base.ends_with('_') {
            base.push('_');
        }
    }
    let mut base: String = base.trim_matches('_').chars().take(schema::ID_CHARS - 4).collect();
    base = base.trim_end_matches('_').to_owned();
    if base.is_empty() {
        base = fallback.to_owned();
    }
    let mut id = base.clone();
    let mut next = 2;
    while taken(&id) {
        id = format!("{base}_{next}");
        next += 1;
    }
    id
}

/// One line of the author's text, as kept, if it's within `most` characters.
fn line(text: &str, most: usize) -> Option<String> {
    let text = schema::cleaned(text, true);
    (text.chars().count() <= most).then_some(text)
}

/// A cap as asked: 0 for the ceiling; past it, refused (invariant 4).
fn cap(asked: u64, ceiling: usize, what: &str, label: &str) -> Result<usize, String> {
    match usize::try_from(asked) {
        Ok(0) => Ok(ceiling),
        Ok(asked) if asked <= ceiling => Ok(asked),
        _ => Err(format!(
            "\"{label}\" can hold at most {} {what}. A report type can lower that limit but not raise it.",
            schema::thousands(i64::try_from(ceiling).unwrap_or(i64::MAX))
        )),
    }
}

/// Checks a definition against every rule (1–10, but for who may make
/// it, which the caller checks): its name, fields and outcomes as kept,
/// or the rule it breaks, in Claktory's words.
pub(crate) fn validate(draft: &Draft) -> Result<(String, Vec<Field>, Vec<Outcome>), String> {
    let name = line(&draft.name, schema::NAME_CHARS).filter(|name| !name.is_empty());
    let name = name.ok_or_else(|| format!("A report type needs a name of up to {} characters.", schema::NAME_CHARS))?;
    if draft.fields.is_empty() {
        return Err("A report type needs at least one field.".to_owned());
    }
    if draft.fields.len() > schema::FIELDS {
        return Err(format!("A report type can have at most {} fields.", schema::FIELDS));
    }
    let given: Vec<&str> = draft.fields.iter().map(|field| field.name.as_str()).filter(|name| !name.is_empty()).collect();
    let mut fields: Vec<Field> = Vec::new();
    for asked in &draft.fields {
        let label = line(&asked.label, schema::LABEL_CHARS).filter(|label| !label.is_empty());
        let label = label.ok_or_else(|| format!("Each field needs a label of up to {} characters.", schema::LABEL_CHARS))?;
        // A label that reads as routing does so whatever id it gets: what
        // the reader sees is the label (on minting "to_2" from "To").
        if asked.name.is_empty() && schema::routing(&minted(&label, "field", &|_| false)) {
            return Err(format!(
                "A field can't be called \"{label}\": it would read as where the report goes, which only the pipe's settings decide. Name it for what it holds."
            ));
        }
        let id = if asked.name.is_empty() {
            minted(&label, "field", &|id| given.contains(&id) || fields.iter().any(|field| field.name == id))
        } else {
            asked.name.clone()
        };
        if !is_id(&id) {
            return Err(format!("A field's id is lowercase letters, digits and _, at most {} characters: \"{id}\" isn't.", schema::ID_CHARS));
        }
        if schema::routing(&id) {
            return Err(format!(
                "A field can't be called \"{id}\": it would read as where the report goes, which only the pipe's settings decide. Name it for what it holds."
            ));
        }
        if fields.iter().any(|field| field.name == id) {
            return Err(format!("Two fields are called \"{id}\"."));
        }
        let kind = match asked.kind.trim() {
            "text" | "text_list" | "bool" | "version" | "attachments" | "date" => self::kind(asked.kind.trim(), Vec::new(), None, None),
            "enum" => {
                let mut choices: Vec<String> = Vec::new();
                for choice in &asked.choices {
                    let choice = choice.trim();
                    if !is_id(choice) {
                        return Err(format!(
                            "\"{label}\"'s choices are lowercase letters, digits and _, at most {} characters each: \"{choice}\" isn't.",
                            schema::ID_CHARS
                        ));
                    }
                    if choices.iter().any(|known| known == choice) {
                        return Err(format!("\"{label}\" has the choice \"{choice}\" twice."));
                    }
                    choices.push(choice.to_owned());
                }
                if choices.is_empty() || choices.len() > schema::CHOICES {
                    return Err(format!("\"{label}\" needs 1 to {} choices.", schema::CHOICES));
                }
                Some(Kind::Enum(choices))
            }
            "number" => {
                let (min, max) = (asked.min.unwrap_or(schema::NUMBER_DEFAULT.0), asked.max.unwrap_or(schema::NUMBER_DEFAULT.1));
                if min < -schema::NUMBER_ABS || max > schema::NUMBER_ABS || min > max {
                    return Err(format!(
                        "\"{label}\" needs a range within {} to {}, its smallest no bigger than its largest.",
                        schema::thousands(-schema::NUMBER_ABS),
                        schema::thousands(schema::NUMBER_ABS)
                    ));
                }
                Some(Kind::Number { min, max })
            }
            _ => None,
        };
        let kind = kind.ok_or_else(|| {
            format!("\"{label}\" needs a kind: text, a list of texts, a choice, yes or no, a version, files, a number or a date. There are no others.")
        })?;
        let (max_bytes, max_items) = match &kind {
            Kind::Text => (cap(asked.max_bytes, schema::TEXT_BYTES, "bytes", &label)?, 0),
            Kind::TextList => (cap(asked.max_bytes, schema::ITEM_BYTES, "bytes in each item", &label)?, cap(asked.max_items, schema::LIST_ITEMS, "items", &label)?),
            Kind::Attachments => (0, cap(asked.max_items, schema::ATTACHMENTS, "files", &label)?),
            _ => (0, 0),
        };
        if kind == Kind::Attachments && fields.iter().any(|field| field.kind == Kind::Attachments) {
            return Err("A report type can have one field of files at most.".to_owned());
        }
        let rule = line(&asked.rule, schema::RULE_CHARS).ok_or_else(|| format!("\"{label}\"'s hint can be at most {} characters.", schema::RULE_CHARS))?;
        let mut field = Field { name: id, label, kind, required: asked.required, max_bytes, max_items, rule_written: !rule.is_empty(), rule };
        if field.rule.is_empty() {
            field.rule = field.made_rule();
        }
        fields.push(field);
    }
    if fields.iter().filter(|field| matches!(field.kind, Kind::Number { .. })).count() > schema::NUMBERS {
        return Err(format!("A report type can have at most {} number fields.", schema::NUMBERS));
    }
    if fields.iter().filter(|field| field.kind == Kind::Date).count() > schema::DATES {
        return Err(format!("A report type can have at most {} date fields.", schema::DATES));
    }
    if fields.iter().filter(|field| field.fact()).count() > schema::FACTS {
        return Err(format!("A report type can have at most {} fields that are choices, yes or no, versions, numbers or dates.", schema::FACTS));
    }
    let mut outcomes = Vec::new();
    for asked in &draft.outcomes {
        let outcome = Outcome::of(asked.trim()).ok_or_else(|| {
            let known: Vec<&str> = Outcome::ALL.iter().map(|outcome| outcome.id()).collect();
            format!("What a reader can answer is one of {}.", known.join(", "))
        })?;
        if !outcomes.contains(&outcome) {
            outcomes.push(outcome);
        }
    }
    if outcomes.is_empty() {
        return Err("A report type needs at least one answer its reader can give.".to_owned());
    }
    outcomes.sort();
    Ok((name, fields, outcomes))
}

/// The fields `fields` adds or widens over `before`: a new field, a higher
/// cap, more choices, a wider range, a fact made words, or another kind of
/// fact. Outcomes go back, not across, so they never widen.
pub(crate) fn widened(before: &Schema, fields: &[Field]) -> Vec<String> {
    fields
        .iter()
        .filter(|field| {
            let Some(old) = before.field(&field.name) else { return true };
            match (&old.kind, &field.kind) {
                (Kind::Text, Kind::Text) => field.max_bytes > old.max_bytes,
                (Kind::TextList, Kind::TextList) => field.max_bytes > old.max_bytes || field.max_items > old.max_items,
                (Kind::Attachments, Kind::Attachments) => field.max_items > old.max_items,
                (Kind::Enum(old), Kind::Enum(new)) => new.iter().any(|choice| !old.contains(choice)),
                (Kind::Number { min: old_min, max: old_max }, Kind::Number { min, max }) => min < old_min || max > old_max,
                (old, new) if old == new => false,
                // Words made a fact narrows; anything else that changes kind widens.
                (old, new) => !(old.words() && !new.words()),
            }
        })
        .map(|field| field.name.clone())
        .collect()
}

/// Defines a type (`draft.id` empty), or edits one into its next version
/// (`base`: the version the editor started from; None takes the current).
/// `by`: "" for the person, else the admin agent, untainted (the caller
/// checks). An edit that changes nothing keeps the current version.
pub(crate) fn save(factory: &Arc<Factory>, draft: &Draft, base: Option<u32>, by: &str) -> Result<Arc<Schema>, String> {
    let (name, fields, outcomes) = validate(draft)?;
    let known = all(factory);
    let made = {
        let mut db = factory.store.db();
        let transaction = db.transaction().map_err(|error| error.to_string())?;
        let latest = |id: &str| -> rusqlite::Result<Option<(u32, bool)>> {
            transaction
                .query_row(
                    "SELECT version, deleted FROM pipe_handoff_types WHERE id = ?1 ORDER BY version DESC LIMIT 1",
                    [id],
                    crate::store::row2,
                )
                .optional()
        };
        let (id, before) = if draft.id.is_empty() {
            let count: i64 = transaction
                .query_row("SELECT COUNT(DISTINCT id) FROM pipe_handoff_types WHERE deleted = 0", [], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            if count >= schema::TYPES as i64 {
                return Err(format!("This Claktory already has {} report types, the most it can keep. Delete one first.", schema::TYPES));
            }
            let id = minted(&name, "type", &|id| known.iter().any(|schema| schema.id == id) || latest(id).ok().flatten().is_some());
            (id, None)
        } else {
            let id = draft.id.clone();
            let Some((version, false)) = latest(&id).map_err(|error| error.to_string())? else {
                return Err(format!("There's no report type \"{id}\"."));
            };
            if base.is_some_and(|base| base != version) {
                return Err(format!("This report type changed after you opened it. Open it again to see version {version}."));
            }
            let before = known.iter().find(|schema| schema.id == id && schema.version == version).cloned();
            let before = before.ok_or_else(|| format!("Report type \"{id}\" can't be read."))?;
            (id, Some(before))
        };
        if let Some(before) = &before
            && before.name == name
            && before.fields == fields
            && before.outcomes == outcomes
        {
            return Ok(before.clone());
        }
        let schema = Schema {
            id: id.clone(),
            version: before.as_ref().map_or(1, |before| before.version + 1),
            name,
            widened: before.as_ref().map(|before| widened(before, &fields)).unwrap_or_default(),
            fields,
            outcomes,
            builtin: before.as_ref().is_some_and(|before| before.builtin),
            by: by.to_owned(),
            at: now_ms(),
            deleted: false,
        };
        insert(&transaction, &schema).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO pipe_handoff_changes (kind, target, old, new, by, at, kept) VALUES ('type', ?1, ?2, ?3, ?4, ?5, 0)",
                params![id, before.as_ref().map(|before| before.version.to_string()).unwrap_or_default(), schema.version.to_string(), by, schema.at],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Arc::new(schema)
    };
    forget(factory);
    if made.version > 1 {
        super::types_changed(factory, &made.id);
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(made)
}

/// Where `id` is on: pipes (by the person's name for them, or their id) and
/// projects' new pipes.
fn enabled_on(factory: &Factory, id: &str) -> Vec<String> {
    let mut places = Vec::new();
    for (kind, target, forward) in super::forwards(factory) {
        if !forward.types.iter().any(|type_id| type_id == id) {
            continue;
        }
        if kind == "pipe" {
            match factory.pipes.get(&target) {
                Some(record) if record.state != crate::pipes::State::Closed => places.push(super::label(&record)),
                _ => {}
            }
        } else {
            let project = factory.lock().config.project(&target).map(|project| project.name.clone());
            if let Some(project) = project {
                places.push(format!("{project}'s new pipes"));
            }
        }
    }
    places
}

/// Deletes a type: never a built-in one, nor one a pipe or a project's new
/// pipes can still forward. Its versions stay, for its reports.
pub(crate) fn delete(factory: &Arc<Factory>, id: &str, by: &str) -> Result<(), String> {
    let schema = current(factory, id).ok_or_else(|| format!("There's no report type \"{id}\"."))?;
    if schema.builtin {
        return Err(format!("\"{}\" is built in: you can change it, but not delete it.", schema.name));
    }
    let places = enabled_on(factory, id);
    if !places.is_empty() {
        let places = factory_agent::text::list_and(&places);
        return Err(format!("“{}” is still forwarded by {places}. Turn it off there first.", schema.name));
    }
    {
        let mut db = factory.store.db();
        let transaction = db.transaction().map_err(|error| error.to_string())?;
        transaction.execute("UPDATE pipe_handoff_types SET deleted = 1 WHERE id = ?1", [id]).map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO pipe_handoff_changes (kind, target, old, new, by, at, kept) VALUES ('type', ?1, ?2, '', ?3, ?4, 0)",
                params![id, schema.version.to_string(), by, now_ms()],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
    }
    forget(factory);
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// SaveReportType, from the person.
pub(crate) fn save_request(factory: &Arc<Factory>, request: &v1::SaveReportType) -> Result<(), RequestError> {
    let Some(asked) = &request.r#type else { return Err(RequestError::Remote("Say which report type.".to_owned())) };
    let draft = Draft::from_proto(asked);
    let base = (!draft.id.is_empty()).then_some(request.base_version);
    save(factory, &draft, base, "").map(|_| ()).map_err(RequestError::Remote)
}

/// DeleteReportType, from the person.
pub(crate) fn delete_request(factory: &Arc<Factory>, request: &v1::DeleteReportType) -> Result<(), RequestError> {
    delete(factory, request.id.trim(), "").map_err(RequestError::Remote)
}

/// What a type crosses with, in a line each, for Rivet's tool.
pub(crate) fn described(schema: &Schema) -> String {
    use std::fmt::Write as _;
    let escape = crate::pipes::agents::escape;
    let mut text = format!(
        "\"{}\" ({}, version {}{}): its reader can answer {}.\n",
        escape(&schema.name),
        schema.id,
        schema.version,
        if schema.builtin { ", built in" } else { "" },
        schema.outcomes.iter().map(|outcome| outcome.id()).collect::<Vec<_>>().join(", ")
    );
    for field in &schema.fields {
        let kind = match &field.kind {
            Kind::Enum(choices) => format!("enum [{}]", choices.join(", ")),
            Kind::Number { min, max } => format!("number {min}..{max}"),
            Kind::Text => format!("text ≤{} bytes", field.max_bytes),
            Kind::TextList => format!("text_list ≤{} × {} bytes", field.max_items, field.max_bytes),
            Kind::Attachments => format!("attachments ≤{}", field.max_items),
            other => other.id().to_owned(),
        };
        let _ = writeln!(
            text,
            "- {}: \"{}\", {kind}{}, {}",
            field.name,
            escape(&field.label),
            if field.required { ", required" } else { "" },
            if field.fact() { "a fact" } else { "their words (marks the reader)" }
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(fields: serde_json::Value) -> Draft {
        serde_json::from_value(serde_json::json!({"name": "Recipe", "fields": fields, "outcomes": ["accepted"]})).expect("draft")
    }

    fn refused(fields: serde_json::Value) -> String {
        validate(&draft(fields)).expect_err("refused")
    }

    #[test]
    fn a_type_is_kept_as_its_author_meant_within_the_ceilings() {
        let (name, fields, outcomes) = validate(&Draft {
            outcomes: vec!["needs_info".into(), "accepted".into(), "accepted".into()],
            ..draft(serde_json::json!([
                {"label": "Dish name", "kind": "text", "required": true, "max_bytes": 200},
                {"label": "Steps", "kind": "text_list"},
                {"name": "serves", "label": "Serves", "kind": "number", "min": 1, "max": 12},
                {"label": "Course", "kind": "enum", "choices": ["starter", "main"], "rule": "Which course\u{202E}"},
                {"label": "Cooked on", "kind": "date"},
                {"label": "Photos", "kind": "attachments", "max_items": 2},
            ]))
        })
        .expect("taken");
        assert_eq!(name, "Recipe");
        assert_eq!(outcomes, vec![Outcome::Accepted, Outcome::NeedsInfo]);
        let ids: Vec<&str> = fields.iter().map(|field| field.name.as_str()).collect();
        assert_eq!(ids, ["dish_name", "steps", "serves", "course", "cooked_on", "photos"]);
        assert_eq!((fields[0].max_bytes, fields[0].rule.as_str(), fields[0].rule_written), (200, "One line, up to 200 bytes", false));
        assert_eq!((fields[1].max_bytes, fields[1].max_items), (schema::ITEM_BYTES, schema::LIST_ITEMS));
        assert_eq!(fields[2].kind, Kind::Number { min: 1, max: 12 });
        assert_eq!((fields[3].rule.as_str(), fields[3].rule_written), ("Which course", true));
        assert_eq!(fields[5].max_items, 2);
        let facts: Vec<bool> = fields.iter().map(Field::fact).collect();
        assert_eq!(facts, [false, false, true, true, true, false]);
    }

    #[test]
    fn rule_1_the_kinds_are_a_closed_set() {
        for kind in ["object", "json", "any", "raw", "map", "", "Text"] {
            assert!(refused(serde_json::json!([{"label": "X", "kind": kind}])).contains("There are no others"), "{kind}");
        }
    }

    #[test]
    fn rule_2_whether_a_field_is_words_is_its_kind_not_a_flag() {
        // A proto field saying it's a fact is ignored: text is words.
        let asked = v1::ReportSchema {
            name: "X".into(),
            fields: vec![v1::ReportField { label: "Note".into(), r#type: v1::ReportFieldType::Text as i32, fact: true, ..Default::default() }],
            outcomes: vec![v1::PipeReportState::Accepted as i32],
            ..Default::default()
        };
        let (_, fields, _) = validate(&Draft::from_proto(&asked)).expect("taken");
        assert!(!fields[0].fact());
    }

    #[test]
    fn rule_3_facts_have_a_bounded_width() {
        let choices: Vec<String> = (0..33).map(|index| format!("c{index}")).collect();
        assert!(refused(serde_json::json!([{"label": "X", "kind": "enum", "choices": choices}])).contains("1 to 32 choices"));
        assert!(refused(serde_json::json!([{"label": "X", "kind": "enum", "choices": []}])).contains("1 to 32 choices"));
        for choice in ["Main", "two words", "é", "a".repeat(41).as_str(), "a-b"] {
            assert!(refused(serde_json::json!([{"label": "X", "kind": "enum", "choices": [choice]}])).contains("choices are lowercase"), "{choice}");
        }
        assert!(refused(serde_json::json!([{"label": "X", "kind": "enum", "choices": ["a", "a"]}])).contains("twice"));
        let facts: Vec<serde_json::Value> = (0..13).map(|index| serde_json::json!({"label": format!("F{index}"), "kind": "bool"})).collect();
        assert!(refused(serde_json::Value::Array(facts)).contains("at most 12 fields that are choices"));
        for (min, max) in [(-1_000_000_001_i64, 0_i64), (0, 1_000_000_001), (5, 4)] {
            assert!(refused(serde_json::json!([{"label": "X", "kind": "number", "min": min, "max": max}])).contains("range"), "{min} {max}");
        }
    }

    #[test]
    fn rule_3_numbers_and_dates_are_few_and_numbers_small_unless_asked() {
        let numbers: Vec<serde_json::Value> = (0..5).map(|index| serde_json::json!({"label": format!("N{index}"), "kind": "number"})).collect();
        assert!(refused(serde_json::Value::Array(numbers[..5].to_vec())).contains("at most 4 number fields"));
        assert!(validate(&draft(serde_json::Value::Array(numbers[..4].to_vec()))).is_ok());
        let dates: Vec<serde_json::Value> = (0..3).map(|index| serde_json::json!({"label": format!("D{index}"), "kind": "date"})).collect();
        assert!(refused(serde_json::Value::Array(dates[..3].to_vec())).contains("at most 2 date fields"));
        assert!(validate(&draft(serde_json::Value::Array(dates[..2].to_vec()))).is_ok());
        let (_, fields, _) = validate(&draft(serde_json::json!([{"label": "Count", "kind": "number"}]))).expect("taken");
        assert_eq!((fields[0].kind.clone(), fields[0].rule.as_str()), (Kind::Number { min: 0, max: 1_000_000 }, "A whole number from 0 to 1,000,000"));
        let (_, fields, _) = validate(&draft(serde_json::json!([{"label": "Delta", "kind": "number", "min": -1_000_000_000_i64, "max": 1_000_000_000}]))).expect("taken");
        assert_eq!(fields[0].kind, Kind::Number { min: -schema::NUMBER_ABS, max: schema::NUMBER_ABS });
    }

    #[test]
    fn rule_4_caps_only_go_down() {
        assert!(refused(serde_json::json!([{"label": "X", "kind": "text", "max_bytes": 4097}])).contains("at most 4,096 bytes"));
        assert!(refused(serde_json::json!([{"label": "X", "kind": "text_list", "max_items": 21}])).contains("at most 20 items"));
        assert!(refused(serde_json::json!([{"label": "X", "kind": "text_list", "max_bytes": 1025}])).contains("1,024 bytes in each item"));
        assert!(refused(serde_json::json!([{"label": "X", "kind": "attachments", "max_items": 4}])).contains("at most 3 files"));
        assert!(refused(serde_json::json!([{"label": "A", "kind": "attachments"}, {"label": "B", "kind": "attachments"}])).contains("one field of files"));
        let many: Vec<serde_json::Value> = (0..21).map(|index| serde_json::json!({"label": format!("T{index}"), "kind": "text"})).collect();
        assert!(refused(serde_json::Value::Array(many)).contains("at most 20 fields"));
    }

    #[test]
    fn rule_5_no_field_addresses_anyone() {
        for id in schema::RESERVED {
            assert!(refused(serde_json::json!([{"name": id, "label": "X", "kind": "text"}])).contains("where the report goes"), "{id}");
        }
        for id in ["Title", "a b", "x".repeat(41).as_str(), "naïve"] {
            assert!(refused(serde_json::json!([{"name": id, "label": "X", "kind": "text"}])).contains("lowercase letters"), "{id}");
        }
        // Any id with a routing word in it, or ending "_to", given or made
        // from its label (before any _2 is added).
        for id in [
            "send_to", "forward_to", "cc_list", "link", "agent_name", "my_pipe", "recipients", "web_url", "urls", "links", "pipes", "agents",
            "email", "emails", "webhook_url", "endpoint", "destination",
        ] {
            assert!(refused(serde_json::json!([{"name": id, "label": "X", "kind": "text"}])).contains("where the report goes"), "{id}");
        }
        for label in ["To", "recipient", "URL", "Forward", " Reply-to ", "Send to", "Link"] {
            assert!(refused(serde_json::json!([{"label": label, "kind": "text"}])).contains("where the report goes"), "{label}");
        }
        let (_, fields, _) = validate(&draft(serde_json::json!([{"label": "Time to fix", "kind": "text"}, {"name": "toast", "label": "Toast", "kind": "text"}]))).expect("taken");
        assert_eq!((fields[0].name.as_str(), fields[1].name.as_str()), ("time_to_fix", "toast"));
        // Common words that only sometimes route stay usable.
        for id in ["notify", "target", "send_now", "file_path", "project_name", "status", "process"] {
            assert!(validate(&draft(serde_json::json!([{"name": id, "label": "X", "kind": "text"}]))).is_ok(), "{id}");
        }
        assert!(refused(serde_json::json!([{"name": "a", "label": "X", "kind": "text"}, {"name": "a", "label": "Y", "kind": "text"}])).contains("Two fields"));
    }

    #[test]
    fn rule_9_its_words_are_bounded() {
        assert!(validate(&Draft { name: "n".repeat(61), ..draft(serde_json::json!([{"label": "X", "kind": "text"}])) }).expect_err("refused").contains("60"));
        assert!(validate(&Draft { name: " \u{200B}".into(), ..draft(serde_json::json!([{"label": "X", "kind": "text"}])) }).is_err());
        assert!(refused(serde_json::json!([{"label": "l".repeat(81), "kind": "text"}])).contains("80"));
        assert!(refused(serde_json::json!([{"label": "X", "kind": "text", "rule": "r".repeat(201)}])).contains("200"));
        assert!(validate(&Draft { outcomes: vec![], ..draft(serde_json::json!([{"label": "X", "kind": "text"}])) }).is_err());
        assert!(validate(&Draft { outcomes: vec!["reply".into()], ..draft(serde_json::json!([{"label": "X", "kind": "text"}])) }).is_err());
    }

    #[test]
    fn the_built_in_types_keep_every_rule() {
        for builtin in schema::builtins() {
            let fields: Vec<DraftField> = builtin
                .fields
                .iter()
                .map(|field| {
                    let stored = Stored::of(field);
                    DraftField {
                        name: stored.name,
                        label: stored.label,
                        kind: stored.kind,
                        choices: stored.choices,
                        required: stored.required,
                        max_bytes: stored.max_bytes as u64,
                        max_items: stored.max_items as u64,
                        min: stored.min,
                        max: stored.max,
                        rule: if field.rule_written { stored.rule } else { String::new() },
                    }
                })
                .collect();
            let outcomes = builtin.outcomes.iter().map(|outcome| outcome.id().to_owned()).collect();
            let (name, kept, outcomes) =
                validate(&Draft { id: builtin.id.clone(), name: builtin.name.clone(), fields, outcomes }).unwrap_or_else(|error| panic!("{}: {error}", builtin.id));
            assert_eq!((name, kept, outcomes), (builtin.name.clone(), builtin.fields.clone(), builtin.outcomes.clone()), "{}", builtin.id);
        }
    }

    #[test]
    fn widening_is_what_lets_more_cross() {
        let before = schema::builtins().into_iter().next().expect("Bug report");
        let mut fields = before.fields.clone();
        assert!(widened(&before, &fields).is_empty());
        fields[0].max_bytes = 100; // title narrower
        fields[1].kind = Kind::Bool; // words made a fact: narrower
        if let Kind::Enum(choices) = &mut fields[4].kind {
            choices.pop(); // platform: fewer choices
        }
        assert!(widened(&before, &fields).is_empty());
        fields[2].max_bytes = 2000; // expected: higher cap
        fields[3].max_items = 11; // steps: more items
        if let Kind::Enum(choices) = &mut fields[6].kind {
            choices.push("billing".into()); // area: a new choice
        }
        fields[7].kind = Kind::Text; // severity: a fact made words
        fields.push(Field { name: "contact".into(), label: "Contact".into(), kind: Kind::Text, required: false, max_bytes: 10, max_items: 0, rule: String::new(), rule_written: false });
        assert_eq!(widened(&before, &fields), ["expected", "steps", "area", "severity", "contact"]);
    }

    #[test]
    fn ids_are_made_from_words() {
        let none = |_: &str| false;
        assert_eq!(minted("Frågor & svar!", "type", &none), "fragor_svar");
        assert_eq!(minted("日本", "type", &none), "type");
        assert_eq!(minted("Bug report", "type", &|id| id == "bug_report"), "bug_report_2");
        assert!(is_id(&minted(&"Long name ".repeat(10), "type", &none)));
    }
}
