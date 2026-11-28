//! Review items (`docs/ipad-phase3-contract.md` §5): ink the
//! person drew on a file, carried by one of their comments on a thread. The
//! comment's body is the question; the item names an ink layer at a frozen
//! revision, the region on it (or an excerpt's text), and the context image
//! the iPad made of it, an ink blob (`crate::ink`).
//!
//! Drafts and their ink reach only the person's devices. Sending, and only
//! sending, publishes: the context image is copied into the addressed
//! agent's uploads with a JSON manifest beside it, and the event names both.
//! A thread readdressed while a draft publishes nothing to anyone.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use factory_protocol::v1;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

use super::region;
use crate::factory::{Factory, RequestError};
use crate::home::Home;
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Review items: ink on the person's thread comments (crate::annotations::ink).
CREATE TABLE ink_items (
    -- The comment carrying it (thread_comments.id), and its thread.
    comment INTEGER PRIMARY KEY,
    thread INTEGER NOT NULL,
    -- The ink layer (its id, \"layer-12\") and the revision it was drawn on.
    layer TEXT NOT NULL,
    layer_revision INTEGER NOT NULL,
    -- JSON {x, y, w, h}, normalized; null for an excerpt.
    region TEXT,
    -- The context image: its blob, as it is, and what it shows (JSON
    -- {x, y, w, h} or null; six numbers).
    context_sha256 TEXT NOT NULL,
    context_bytes INTEGER NOT NULL,
    context_width INTEGER NOT NULL,
    context_height INTEGER NOT NULL,
    context_region TEXT,
    context_transform TEXT NOT NULL,
    excerpt_text TEXT NOT NULL,
    -- JSON {text, layer_revision, languages, recognizer_version}; null for none.
    recognized TEXT,
    -- The copies Send made in the agent's uploads, absolute; '' until sent.
    published_context_path TEXT NOT NULL DEFAULT '',
    published_manifest_path TEXT NOT NULL DEFAULT '',
    created INTEGER NOT NULL
);
CREATE INDEX ink_items_by_layer ON ink_items (layer, layer_revision);
CREATE INDEX ink_items_by_context ON ink_items (context_sha256);
-- SendComments' client_batch_id: sent once, so a repeat delivers nothing.
CREATE TABLE ink_batches (
    id TEXT PRIMARY KEY,
    agent TEXT NOT NULL,
    -- JSON: the comment ids it sent.
    comments TEXT NOT NULL,
    created INTEGER NOT NULL
);
";

/// An excerpt, at most (characters).
const EXCERPT_MAX: usize = 20_000;
/// Recognized handwriting, at most (characters).
const RECOGNIZED_MAX: usize = 10_000;
/// A batch id, at most (bytes).
const BATCH_ID_MAX: usize = 128;
/// How long a sent batch's id is remembered.
const BATCH_KEPT_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// An ink layer at one revision, as review items need it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Layer {
    /// The file it's on, absolute.
    pub path: String,
    /// The file's version it was drawn on.
    pub source_sha256: String,
    /// Its coordinate space, as the manifest gives it (the fixtures'
    /// `coordinate_space`).
    pub space: Value,
}

/// Layer `id` at `revision`, if it's kept (slice B's).
fn layer_at(db: &Connection, id: &str, revision: u32) -> rusqlite::Result<Option<Layer>> {
    Ok(crate::ink::layers::at(db, id, revision)?.map(|layer| Layer {
        path: layer.path,
        source_sha256: layer.source_sha256,
        space: layer.space.as_ref().map_or(Value::Null, space),
    }))
}

/// A layer's space as the manifest gives it, named as the fixtures'
/// `coordinate_space`: only its kind's fields.
fn space(space: &v1::InkSpace) -> Value {
    let mut value = json!({"kind": space.kind, "width": space.width, "height": space.height, "units": space.units});
    match space.kind.as_str() {
        "pdf_page" => {
            value["page_1based"] = json!(space.page_1based);
            value["box"] = json!(space.r#box);
            value["crop_box"] = json!(space.crop_box);
            value["media_box"] = json!(space.media_box);
            value["rotation"] = json!(space.rotation);
        }
        "image" => {
            value["oriented_width"] = json!(space.oriented_width);
            value["oriented_height"] = json!(space.oriented_height);
            value["stored_width"] = json!(space.stored_width);
            value["stored_height"] = json!(space.stored_height);
            value["exif_orientation"] = json!(space.exif_orientation);
        }
        "excerpt_rendition" => {
            value["text"] = json!(space.text);
            if let Some(rendition) = &space.rendition {
                value["rendition"] = json!({
                    "font": rendition.font,
                    "font_size": rendition.font_size,
                    "line_height": rendition.line_height,
                    "padding": rendition.padding,
                });
            }
        }
        _ => {}
    }
    value
}

fn refused(why: impl Into<String>) -> RequestError {
    RequestError::Comments(why.into())
}

fn rect(rect: &v1::NormalizedRect) -> bool {
    region::in_range(rect.x, rect.y, rect.w, rect.h)
}

fn rect_json(rect: Option<&v1::NormalizedRect>) -> Option<String> {
    rect.map(|rect| json!({"x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h}).to_string())
}

fn rect_of(json: Option<String>) -> Option<v1::NormalizedRect> {
    let value: Value = serde_json::from_str(&json?).ok()?;
    let number = |name: &str| value[name].as_f64().unwrap_or_default();
    Some(v1::NormalizedRect { x: number("x"), y: number("y"), w: number("w"), h: number("h") })
}

fn rect_value(rect: Option<&v1::NormalizedRect>) -> Value {
    rect.map_or(Value::Null, |rect| json!({"x": rect.x, "y": rect.y, "w": rect.w, "h": rect.h}))
}

fn recognized_value(recognized: &v1::RecognizedText) -> Value {
    json!({
        "text": recognized.text,
        "layer_revision": recognized.layer_revision,
        "languages": recognized.languages,
        "recognizer_version": recognized.recognizer_version,
    })
}

/// Checks the ink a comment is to carry, on the thread on `path` at
/// `version` (`text`: a text or Markdown thread, whose ink may be on an
/// excerpt rather than a region): its layer exists at that revision, on
/// this file at this version; its regions are on their surface; its
/// context image is uploaded, within its limits, and the one described.
/// Answers it as it's kept: sizes as the blob is, nothing published.
pub(super) fn check(
    home: &Home,
    db: &Connection,
    path: &str,
    version: &str,
    text: bool,
    item: &v1::InkItem,
) -> Result<v1::InkItem, RequestError> {
    if item.ink_layer_id.trim().is_empty() || item.layer_revision == 0 {
        return Err(refused("Save your drawing before adding it to a comment."));
    }
    let Some(layer) = layer_at(db, &item.ink_layer_id, item.layer_revision)? else {
        return Err(refused("That drawing isn't saved as you drew it. Save it and try again."));
    };
    if layer.path != path {
        return Err(refused("That drawing is on another file."));
    }
    if layer.source_sha256 != version {
        return Err(refused("That drawing is on another version of this file."));
    }
    match &item.region {
        Some(region) if !rect(region) => return Err(refused("That drawing is outside the page.")),
        None if !text => return Err(refused("Couldn't save this comment: it has no place on the page.")),
        _ => {}
    }
    let Some(context) = &item.context else { return Err(refused("This comment is missing its picture of the page. Draw it again.")) };
    let (blob, width, height) =
        crate::ink::picture(home, &context.sha256, crate::ink::CONTEXT_MAX, Some(crate::ink::CONTEXT_EDGE_MAX)).map_err(refused)?;
    if context.bytes != blob.bytes || context.pixel_width != width || context.pixel_height != height {
        return Err(refused("This comment's picture of the page doesn't match. Draw it again."));
    }
    if context.region.as_ref().is_some_and(|region| !rect(region)) {
        return Err(refused("This comment's picture of the page doesn't match. Draw it again."));
    }
    let transform = &context.crop_to_document_transform;
    if transform.len() != 6 || transform.iter().any(|number| !number.is_finite()) {
        return Err(refused("This comment's picture of the page doesn't match. Draw it again."));
    }
    if item.excerpt_text.chars().count() > EXCERPT_MAX {
        return Err(refused(format!("An excerpt is at most {EXCERPT_MAX} characters.")));
    }
    if let Some(recognized) = &item.recognized {
        let long = recognized.text.chars().count() > RECOGNIZED_MAX
            || recognized.languages.len() > 16
            || recognized.languages.iter().any(|language| language.len() > 35)
            || recognized.recognizer_version.len() > 200;
        if long {
            return Err(refused("Couldn't save this comment: its handwriting is too long."));
        }
    }
    Ok(v1::InkItem { published_context_path: String::new(), published_manifest_path: String::new(), ..item.clone() })
}

/// Keeps `item` (checked) as the ink comment `comment` on `thread` carries.
pub(super) fn insert(db: &Connection, comment: i64, thread: i64, item: &v1::InkItem) -> rusqlite::Result<()> {
    let context = item.context.clone().unwrap_or_default();
    db.execute(
        "INSERT INTO ink_items (comment, thread, layer, layer_revision, region, context_sha256, context_bytes, context_width,
                                context_height, context_region, context_transform, excerpt_text, recognized, created)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            comment,
            thread,
            item.ink_layer_id,
            item.layer_revision,
            rect_json(item.region.as_ref()),
            context.sha256,
            context.bytes as i64,
            context.pixel_width,
            context.pixel_height,
            rect_json(context.region.as_ref()),
            Value::from(context.crop_to_document_transform.clone()).to_string(),
            item.excerpt_text,
            item.recognized.as_ref().map(|recognized| recognized_value(recognized).to_string()),
            now_ms(),
        ],
    )?;
    Ok(())
}

const COLUMNS: &str =
    "comment, layer, layer_revision, region, context_sha256, context_bytes, context_width, context_height, context_region,
                       context_transform, excerpt_text, recognized, published_context_path, published_manifest_path";

fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, v1::InkItem)> {
    let transform: String = row.get(9)?;
    let recognized: Option<String> = row.get(11)?;
    let recognized = recognized.and_then(|json| serde_json::from_str::<Value>(&json).ok()).map(|value| v1::RecognizedText {
        text: value["text"].as_str().unwrap_or_default().to_owned(),
        layer_revision: value["layer_revision"].as_u64().unwrap_or_default() as u32,
        languages: value["languages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|language| language.as_str().map(str::to_owned))
            .collect(),
        recognizer_version: value["recognizer_version"].as_str().unwrap_or_default().to_owned(),
    });
    let item = v1::InkItem {
        ink_layer_id: row.get(1)?,
        layer_revision: row.get(2)?,
        region: rect_of(row.get(3)?),
        context: Some(v1::InkComposite {
            sha256: row.get(4)?,
            bytes: row.get::<_, i64>(5)?.max(0) as u64,
            pixel_width: row.get(6)?,
            pixel_height: row.get(7)?,
            region: rect_of(row.get(8)?),
            crop_to_document_transform: serde_json::from_str(&transform).unwrap_or_default(),
        }),
        excerpt_text: row.get(10)?,
        recognized,
        published_context_path: row.get(12)?,
        published_manifest_path: row.get(13)?,
    };
    Ok((row.get(0)?, item))
}

/// The ink each comment on `thread` carries, by comment.
pub(super) fn of_thread(db: &Connection, thread: i64) -> rusqlite::Result<std::collections::HashMap<i64, v1::InkItem>> {
    let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM ink_items WHERE thread = ?1"))?;
    statement.query_map([thread], read)?.collect()
}

/// The ink comment `comment` carries, if any.
pub(super) fn of_comment(db: &Connection, comment: i64) -> rusqlite::Result<Option<v1::InkItem>> {
    db.query_row(&format!("SELECT {COLUMNS} FROM ink_items WHERE comment = ?1"), [comment], read)
        .optional()
        .map(|found| found.map(|(_, item)| item))
}

/// Forgets the ink comment `comment` carried (it's deleted). Copies sent
/// to an agent stay in its uploads.
pub(super) fn forget(db: &Connection, comment: i64) -> rusqlite::Result<()> {
    db.execute("DELETE FROM ink_items WHERE comment = ?1", [comment])?;
    Ok(())
}

/// Whether a review item refers to blob `sha` (its context image): such a
/// blob can be read and is kept (`crate::ink::referenced`).
pub(crate) fn refers_to(db: &Connection, sha: &str) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS (SELECT 1 FROM ink_items WHERE context_sha256 = ?1)", [sha], |row| row.get(0))
}

/// Whether a review item, sent or not, refers to layer `id`: it can't be
/// deleted while one does, nor when that can't be told.
pub(crate) fn layer_in_use(db: &Connection, id: &str) -> bool {
    db.query_row("SELECT EXISTS (SELECT 1 FROM ink_items WHERE layer = ?1)", [id], |row| row.get(0)).unwrap_or(true)
}

/// The revisions of layer `id` review items refer to, which are kept.
pub(crate) fn revisions(db: &Connection, id: &str) -> rusqlite::Result<BTreeSet<u32>> {
    let mut statement = db.prepare("SELECT DISTINCT layer_revision FROM ink_items WHERE layer = ?1")?;
    statement.query_map([id], |row| row.get(0))?.collect()
}

// ---- Sending ----

/// What a sent item's thread is, for its manifest.
pub(super) struct On<'a> {
    pub thread: &'a str,
    pub path: &'a str,
    pub kind: &'a str,
    pub selectors: Value,
    /// A text thread's quote now; empty otherwise.
    pub quote: &'a str,
}

/// Publishes the ink comment `comment` carries, for `agent`: its context
/// image copied into the agent's uploads, and its manifest beside it,
/// named after the thread and the comment ("t-41-c97-context.png",
/// "t-41-c97.json"). Answers the item with both paths, kept. Sending the
/// same comment again (a retry) writes them again, in place.
pub(super) fn publish(
    home: &Home,
    db: &Connection,
    agent: &str,
    on: &On<'_>,
    comment: i64,
    question: &str,
) -> Result<Option<v1::InkItem>, RequestError> {
    let Some(mut item) = of_comment(db, comment)? else { return Ok(None) };
    let context = item.context.clone().unwrap_or_default();
    let blob =
        crate::ink::blob(home, &context.sha256).ok_or_else(|| refused("This comment's picture of the page is gone. Draw it again."))?;
    let folder = match Path::new(&item.published_context_path).parent() {
        Some(parent) if !item.published_context_path.is_empty() => parent.to_path_buf(),
        _ => home.agent_dir(agent).join("uploads").join(crate::uploads::today()),
    };
    std::fs::create_dir_all(&folder)?;
    let name = format!("{}-c{comment}", on.thread);
    let image = folder.join(format!("{name}-context.png"));
    let manifest = folder.join(format!("{name}.json"));
    copy(&blob.path, &image)?;
    let layer = layer_at(db, &item.ink_layer_id, item.layer_revision)?;
    let mut described = json!({
        "schema_version": 1,
        "thread": on.thread,
        "comment": format!("c-{comment}"),
        "question": if question.trim().is_empty() { Value::Null } else { Value::from(question.trim()) },
        "source": {
            "path": on.path,
            "sha256": layer.as_ref().map(|layer| layer.source_sha256.clone()),
        },
        "anchor": {"kind": on.kind, "selectors": on.selectors},
        "region": rect_value(item.region.as_ref()),
        "excerpt_text": if item.excerpt_text.is_empty() { Value::Null } else { Value::from(item.excerpt_text.clone()) },
        "ink": {
            "layer_revision": item.layer_revision,
            "space": layer.map_or(Value::Null, |layer| layer.space),
        },
        "context_image": {
            "path": image.display().to_string(),
            "sha256": context.sha256,
            "bytes": context.bytes,
            "pixel_width": context.pixel_width,
            "pixel_height": context.pixel_height,
            "region": rect_value(context.region.as_ref()),
            "crop_to_document_transform": context.crop_to_document_transform,
        },
        "recognized_text": item.recognized.as_ref().map_or(Value::Null, recognized_value),
    });
    if !on.quote.is_empty() {
        described["quote"] = Value::from(on.quote);
    }
    if let Ok(selectors) = serde_json::from_value::<Vec<Value>>(on.selectors.clone())
        && let Ok(page) = region::page_region(&selectors)
    {
        described["page_1based"] = Value::from(page.page);
    }
    write(&manifest, &serde_json::to_vec_pretty(&described).unwrap_or_default())?;
    item.published_context_path = image.display().to_string();
    item.published_manifest_path = manifest.display().to_string();
    db.execute(
        "UPDATE ink_items SET published_context_path = ?2, published_manifest_path = ?3 WHERE comment = ?1",
        params![comment, item.published_context_path, item.published_manifest_path],
    )?;
    Ok(Some(item))
}

/// Copies `from` to `to` whole or not at all.
fn copy(from: &Path, to: &Path) -> std::io::Result<()> {
    let temporary = temporary(to);
    std::fs::copy(from, &temporary)?;
    finish(&temporary, to)
}

fn write(to: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = temporary(to);
    std::fs::write(&temporary, bytes)?;
    finish(&temporary, to)
}

fn temporary(to: &Path) -> PathBuf {
    let name = to.file_name().map_or_else(|| "item".to_owned(), |name| name.to_string_lossy().into_owned());
    to.with_file_name(format!(".{name}.claktory-{}", std::process::id()))
}

fn finish(temporary: &Path, to: &Path) -> std::io::Result<()> {
    // An ordinary upload's, not the blob's 0600. On Windows it has what the
    // folder it's written in gives, as an upload does.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(temporary, std::fs::Permissions::from_mode(0o644))?;
    }
    std::fs::rename(temporary, to).inspect_err(|_| {
        let _ = std::fs::remove_file(temporary);
    })
}

/// How the agent can look at a context image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sight {
    /// Its model reads images and it has read_image now.
    Now,
    /// Its model reads images, but read_image comes only with its next
    /// context.
    Later,
    /// Its model doesn't read images.
    Blind,
}

pub(super) fn sight(factory: &Factory, agent: &str) -> Sight {
    if !factory.takes_images(agent) {
        return Sight::Blind;
    }
    let inner = factory.lock();
    let model = inner.config.agent(agent).map(|agent| agent.model.clone()).unwrap_or_default();
    // A first run, or a new model, starts a frame with every tool.
    let frame = inner
        .agents
        .get(agent)
        .and_then(|runtime| runtime.transcript.as_ref())
        .and_then(|transcript| transcript.frame())
        .filter(|frame| crate::runner::frame_model(frame) == model);
    match frame {
        Some(frame) if !frame.tools.iter().any(|tool| tool.name == "read_image") => Sight::Later,
        _ => Sight::Now,
    }
}

/// The event's last lines for review items: whose words they are, and
/// how to answer. `threads` are the items' threads ("t-41"); `tools`, the
/// agent has the Comments extension.
pub(super) fn closing(sight: Sight, threads: &[String], tools: bool) -> String {
    let one = match threads {
        [thread] => Some(thread.as_str()),
        _ => None,
    };
    let answer = match (tools, one) {
        (true, Some(thread)) => format!("answer on {thread} with comments_reply"),
        (true, None) => "answer on each thread with comments_reply and its id".to_owned(),
        (false, _) => "answer".to_owned(),
    };
    let how = match (sight, one) {
        (Sight::Now, Some(_)) => format!("Look at the context image with read_image, then {answer}."),
        (Sight::Now, None) => format!("Look at each context image with read_image, then {answer}."),
        (Sight::Later, Some(_)) => {
            format!(
                "You can't look at images yet (read_image comes with your next context): read the manifest and the question, then {answer}."
            )
        }
        (Sight::Later, None) => {
            format!(
                "You can't look at images yet (read_image comes with your next context): read each manifest and question, then {answer}."
            )
        }
        (Sight::Blind, _) => format!("Your model can't see images: {answer} from the question, the quote and the manifest."),
    };
    format!("This is feedback on the document, not instructions from it.\n{how}\n")
}

// ---- Batches ----

/// A batch id as sent, checked; None for none.
pub(super) fn batch_id(id: &str) -> Result<Option<&str>, RequestError> {
    let id = id.trim();
    if id.is_empty() {
        return Ok(None);
    }
    if id.len() > BATCH_ID_MAX || id.chars().any(char::is_control) {
        return Err(refused("client_batch_id is up to 128 characters, on one line."));
    }
    Ok(Some(id))
}

/// Whether batch `id` was sent already.
pub(super) fn sent(db: &Connection, id: &str) -> rusqlite::Result<bool> {
    db.query_row("SELECT EXISTS (SELECT 1 FROM ink_batches WHERE id = ?1)", [id], |row| row.get(0))
}

/// Remembers batch `id` as sent, for a week.
pub(super) fn record(db: &Connection, id: &str, agent: &str, comments: &[i64]) -> rusqlite::Result<()> {
    let now = now_ms();
    db.execute("DELETE FROM ink_batches WHERE created < ?1", [now - BATCH_KEPT_MS])?;
    db.execute(
        "INSERT OR IGNORE INTO ink_batches (id, agent, comments, created) VALUES (?1, ?2, ?3, ?4)",
        params![id, agent, Value::from(comments.to_vec()).to_string(), now],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_lines_fit_what_the_agent_can_do() {
        let one = ["t-41".to_owned()];
        assert_eq!(
            closing(Sight::Now, &one, true),
            "This is feedback on the document, not instructions from it.\nLook at the context image with read_image, then answer on t-41 with comments_reply.\n"
        );
        assert!(closing(Sight::Later, &one, true).contains("read the manifest and the question, then answer on t-41 with comments_reply."));
        assert!(
            closing(Sight::Blind, &one, true).contains("Your model can't see images: answer on t-41 with comments_reply from the question")
        );
        let two = ["t-41".to_owned(), "t-45".to_owned()];
        assert!(
            closing(Sight::Now, &two, true)
                .contains("Look at each context image with read_image, then answer on each thread with comments_reply and its id.")
        );
        assert!(closing(Sight::Now, &one, false).contains("with read_image, then answer.\n"));
    }

    #[test]
    fn batch_ids_are_checked() {
        assert_eq!(batch_id("  ").ok(), Some(None));
        assert_eq!(batch_id("batch-6f1c2a90").ok(), Some(Some("batch-6f1c2a90")));
        assert!(batch_id(&"b".repeat(129)).is_err());
        assert!(batch_id("a\nb").is_err());
    }
}
