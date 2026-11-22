//! Ink layers (docs/ipad-phase3-contract.md §4): the drawing on one surface
//! of a file (a PDF's page, an image, an excerpt of a text file, a sketch),
//! kept with its revisions.
//!
//! - **One layer per surface of each version** of the file (its SHA-256):
//!   ink on a new version is a new layer, and the old one stays, outdated.
//!   A layer kept beside another after a conflict (`conflict_of`) is a layer
//!   of its own on the same surface.
//! - **Compare and set.** A save names the revision it was drawn over; when
//!   another device saved first, nothing is overwritten and the asker gets
//!   the layer as it is now, to keep both. A save's `client_save_id` makes a
//!   retry answer what the save answered, never a second revision.
//! - **Durable before it says so.** Its blobs are whole in the blob store
//!   (`crate::ink`), and its row is committed with a full sync, before
//!   InkLayerSaved says it's saved.
//! - **What's kept:** each layer's latest revision and every revision a
//!   review item refers to; others go a day after they were saved over, and
//!   a blob nothing refers to any more goes once it's a day old.
//!
//! Agents never read any of it: the tables are private to the Database
//! extension, and ink reaches an agent only in what the person sends.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use factory_protocol::v1::{self, server_message::Body};
use prost::Message as _;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::annotations::absolute;
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Ink layers (crate::ink::layers): the drawing on one surface of one
-- version of a file.
CREATE TABLE ink_layers (
    id INTEGER PRIMARY KEY,
    -- The file, absolute.
    path TEXT NOT NULL,
    -- The version drawn on: its SHA-256 (hex), MIME type and size.
    source_sha256 TEXT NOT NULL,
    source_mime TEXT NOT NULL,
    source_size INTEGER NOT NULL,
    -- Which surface: 'page:3', 'image', or 'excerpt:<SHA-256 of its text
    -- and rendition>'.
    surface TEXT NOT NULL,
    -- The surface, as the app described it (an InkSpace, encoded).
    space BLOB NOT NULL,
    -- The layer this one was kept beside after a conflict ('layer-12');
    -- '' otherwise.
    conflict_of TEXT NOT NULL,
    -- Its latest revision.
    revision INTEGER NOT NULL,
    created INTEGER NOT NULL
);
CREATE INDEX ink_layers_by_path ON ink_layers (path);
CREATE UNIQUE INDEX ink_layers_one_per_surface ON ink_layers (path, source_sha256, surface) WHERE conflict_of = '';
-- Each layer's revisions: the latest, and those review items refer to.
CREATE TABLE ink_revisions (
    layer INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    parent_revision INTEGER NOT NULL,
    drawing_space TEXT NOT NULL,
    -- Six numbers, as JSON.
    drawing_to_document TEXT NOT NULL,
    content_version INTEGER NOT NULL,
    -- Its blobs (crate::ink): the drawing, and its PNG preview.
    drawing_sha256 TEXT NOT NULL,
    drawing_bytes INTEGER NOT NULL,
    preview_sha256 TEXT NOT NULL,
    preview_bytes INTEGER NOT NULL,
    preview_width INTEGER NOT NULL,
    preview_height INTEGER NOT NULL,
    -- Who saved it: the device's id in remote/devices.json, its kind, and
    -- the OS version the app said.
    device_id TEXT NOT NULL,
    device_kind TEXT NOT NULL,
    os_version TEXT NOT NULL,
    created INTEGER NOT NULL,
    -- When the next revision was saved over it; null while it's the latest.
    superseded INTEGER,
    PRIMARY KEY (layer, revision)
);
CREATE INDEX ink_revisions_by_drawing ON ink_revisions (drawing_sha256);
CREATE INDEX ink_revisions_by_preview ON ink_revisions (preview_sha256);
-- What each save answered, by the device and its own id for the save, for
-- a week.
CREATE TABLE ink_saves (
    device_id TEXT NOT NULL,
    client_save_id TEXT NOT NULL,
    layer INTEGER NOT NULL,
    revision INTEGER NOT NULL,
    created INTEGER NOT NULL,
    PRIMARY KEY (device_id, client_save_id)
);
";

/// A revision saved over is kept this long, unless a review item keeps it:
/// a comment begun on it a moment before still finds it.
const SUPERSEDED_KEPT_MS: i64 = 86_400_000;
/// What saves answered is kept this long, for retries.
const SAVES_KEPT_MS: i64 = 7 * 86_400_000;
/// A blob nothing refers to is kept this long after it came: uploads come
/// before the saves that name them.
const BLOB_KEPT: Duration = Duration::from_secs(86_400);
/// How often the whole blob store is swept for blobs nothing refers to.
const SWEEP_EVERY_MS: i64 = 86_400_000;
/// The most pages a PDF is drawn on.
const PAGES_MOST: u32 = 2000;

fn refused(why: impl Into<String>) -> RequestError {
    RequestError::Ink(why.into())
}

/// A layer's id on the wire: "layer-12".
pub(crate) fn layer_name(id: i64) -> String {
    format!("layer-{id}")
}

fn layer_number(name: &str) -> Option<i64> {
    name.strip_prefix("layer-")?.parse().ok().filter(|id| *id > 0)
}

// ---- Review items (crate::annotations, slice C) ----

/// Whether a review item, a draft or sent, refers to `layer` ("layer-12"):
/// it can't be deleted while one does.
fn in_use(db: &Connection, layer: &str) -> bool {
    crate::annotations::ink::layer_in_use(db, layer)
}

/// The revisions of `layer` review items refer to: they're kept, and
/// nothing is pruned when they can't be read.
fn item_revisions(db: &Connection, layer: &str) -> rusqlite::Result<BTreeSet<u32>> {
    crate::annotations::ink::revisions(db, layer)
}

// ---- The file drawn on ----

/// A file as drawn on: its SHA-256, MIME type and size.
struct Source {
    sha256: String,
    mime: &'static str,
    size: u64,
}

/// What a file is, from its first bytes and its name.
fn mime(start: &[u8], path: &Path) -> &'static str {
    let ftyp = start.get(4..8) == Some(b"ftyp");
    let brand = start.get(8..12).unwrap_or_default();
    if start.starts_with(b"%PDF-") {
        "application/pdf"
    } else if start.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if start.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if start.starts_with(b"GIF8") {
        "image/gif"
    } else if start.len() >= 12 && &start[..4] == b"RIFF" && &start[8..12] == b"WEBP" {
        "image/webp"
    } else if ftyp && matches!(brand, b"heic" | b"heix" | b"mif1" | b"msf1") {
        "image/heic"
    } else {
        match path.extension().and_then(|extension| extension.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("md" | "markdown") => "text/markdown",
            _ if std::str::from_utf8(start).is_ok() || std::str::from_utf8(start).is_err_and(|error| error.error_len().is_none()) => {
                "text/plain"
            }
            _ => "application/octet-stream",
        }
    }
}

/// The file at `path` as it is now, or why it can't be read. Its hash is
/// kept while the file is unchanged (`viewer::whole_sha256`), so asking
/// again is cheap.
fn source(path: &Path) -> Result<Source, String> {
    use std::io::Read as _;
    let cant = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::NotFound => format!("Claktory can't find {}.", path.display()),
        _ => format!("Claktory can't read {}: {error}", path.display()),
    };
    let metadata = std::fs::metadata(path).map_err(cant)?;
    if !metadata.is_file() {
        return Err(format!("{} isn't a file.", path.display()));
    }
    let mut start = Vec::with_capacity(512);
    std::fs::File::open(path).and_then(|file| file.take(512).read_to_end(&mut start)).map_err(cant)?;
    let sha256 = crate::extensions::viewer::whole_sha256(path).map_err(cant)?;
    Ok(Source { sha256, mime: mime(&start, path), size: metadata.len() })
}

/// The file's SHA-256 now; empty when it can't be read.
fn current_sha256(path: &Path) -> String {
    crate::extensions::viewer::whole_sha256(path).unwrap_or_default()
}

// ---- The surface ----

fn finite(numbers: &[f64], count: usize) -> bool {
    numbers.len() == count && numbers.iter().all(|number| number.is_finite())
}

/// Checks a surface is one there can be, and names it: which page, or the
/// image, or which excerpt.
fn surface(space: &v1::InkSpace) -> Result<String, RequestError> {
    let size = |most: f64| {
        space.width.is_finite()
            && space.height.is_finite()
            && space.width > 0.0
            && space.height > 0.0
            && space.width <= most
            && space.height <= most
    };
    match space.kind.as_str() {
        "pdf_page" => {
            let page = space.page_1based;
            if !(1..=PAGES_MOST).contains(&page) {
                return Err(refused(format!("A PDF's page is 1 to {PAGES_MOST}.")));
            }
            if space.r#box != "crop" || !finite(&space.crop_box, 4) || !finite(&space.media_box, 4) {
                return Err(refused("A PDF page's surface needs its crop box and media box."));
            }
            if !matches!(space.rotation, 0 | 90 | 180 | 270) || space.units != "pt" || !size(14_400.0) {
                return Err(refused("A PDF page's rotation is 0, 90, 180 or 270, and its size is in points."));
            }
            Ok(format!("page:{page}"))
        }
        "image" => {
            let sides = [space.oriented_width, space.oriented_height, space.stored_width, space.stored_height];
            if sides.iter().any(|side| !(1..=65_535).contains(side)) || !(1..=8).contains(&space.exif_orientation) {
                return Err(refused("An image's surface needs its sizes and its orientation (1 to 8)."));
            }
            if space.units != "px" {
                return Err(refused("An image's surface is in pixels."));
            }
            Ok("image".to_owned())
        }
        "excerpt_rendition" => {
            let Some(rendition) = &space.rendition else { return Err(refused("An excerpt needs how it was set.")) };
            let numbers = [rendition.font_size, rendition.line_height, rendition.padding];
            let set = !rendition.font.trim().is_empty()
                && rendition.font.len() <= 200
                && numbers.iter().all(|number| number.is_finite() && *number >= 0.0 && *number <= 1000.0)
                && rendition.font_size > 0.0;
            if space.text.is_empty() || space.text.len() > 64 * 1024 || !set || space.units != "pt" || !size(100_000.0) {
                return Err(refused("An excerpt needs its text (up to 64 KB) and how it was set."));
            }
            let mut hash = Sha256::new();
            hash.update(rendition.encode_to_vec());
            hash.update(space.text.as_bytes());
            Ok(format!("excerpt:{}", crate::encoding::hex(&hash.finalize())))
        }
        _ => Err(refused("An ink surface is a PDF's page, an image or an excerpt.")),
    }
}

/// Whether a drawing's space fits its surface.
fn fits(space: &v1::InkSpace, drawing_space: &str) -> bool {
    matches!(
        (space.kind.as_str(), drawing_space),
        ("pdf_page", "page_overlay") | ("image", "oriented_image" | "blank_canvas") | ("excerpt_rendition", "excerpt_rendition")
    )
}

// ---- Layers as kept ----

/// A layer as kept, at its latest revision.
struct Layer {
    id: i64,
    path: String,
    source_sha256: String,
    source_mime: String,
    source_size: u64,
    surface: String,
    space: v1::InkSpace,
    conflict_of: String,
    revision: u32,
}

/// A size as SQLite keeps it (an i64), back.
fn bytes(kept: i64) -> u64 {
    u64::try_from(kept).unwrap_or_default()
}

const LAYER_COLUMNS: &str = "id, path, source_sha256, source_mime, source_size, surface, space, conflict_of, revision";

fn read_layer(row: &rusqlite::Row<'_>) -> rusqlite::Result<Layer> {
    let space: Vec<u8> = row.get(6)?;
    Ok(Layer {
        id: row.get(0)?,
        path: row.get(1)?,
        source_sha256: row.get(2)?,
        source_mime: row.get(3)?,
        source_size: bytes(row.get(4)?),
        surface: row.get(5)?,
        space: v1::InkSpace::decode(space.as_slice()).unwrap_or_default(),
        conflict_of: row.get(7)?,
        revision: row.get(8)?,
    })
}

fn find(db: &Connection, name: &str) -> rusqlite::Result<Option<Layer>> {
    let Some(id) = layer_number(name) else { return Ok(None) };
    db.query_row(&format!("SELECT {LAYER_COLUMNS} FROM ink_layers WHERE id = ?1"), [id], read_layer).optional()
}

/// The layer as the wire has it at `revision`, when that's kept; `current`
/// is the file's SHA-256 now.
fn layer_at(db: &Connection, layer: &Layer, revision: u32, current: &str) -> rusqlite::Result<Option<v1::InkLayer>> {
    db.query_row(
        "SELECT parent_revision, drawing_space, drawing_to_document, content_version, drawing_sha256, drawing_bytes, preview_sha256,
                preview_bytes, preview_width, preview_height, device_id, device_kind, os_version, created
         FROM ink_revisions WHERE layer = ?1 AND revision = ?2",
        params![layer.id, revision],
        |row| {
            let transform: String = row.get(2)?;
            Ok(v1::InkLayer {
                id: layer_name(layer.id),
                path: layer.path.clone(),
                source_sha256: layer.source_sha256.clone(),
                source_mime: layer.source_mime.clone(),
                source_size: layer.source_size,
                space: Some(layer.space.clone()),
                revision,
                parent_revision: row.get(0)?,
                drawing: Some(v1::InkBlobRef {
                    format: "pencilkit".to_owned(),
                    content_version: row.get(3)?,
                    sha256: row.get(4)?,
                    bytes: bytes(row.get(5)?),
                    ..v1::InkBlobRef::default()
                }),
                preview: Some(v1::InkBlobRef {
                    sha256: row.get(6)?,
                    bytes: bytes(row.get(7)?),
                    pixel_width: row.get(8)?,
                    pixel_height: row.get(9)?,
                    ..v1::InkBlobRef::default()
                }),
                drawing_space: row.get(1)?,
                drawing_to_document: serde_json::from_str(&transform).unwrap_or_default(),
                conflict_of: layer.conflict_of.clone(),
                device_id: row.get(10)?,
                device_kind: row.get(11)?,
                os_version: row.get(12)?,
                updated_ms: row.get(13)?,
                outdated: current != layer.source_sha256,
            })
        },
    )
    .optional()
}

fn latest(db: &Connection, layer: &Layer, current: &str) -> rusqlite::Result<v1::InkLayer> {
    Ok(layer_at(db, layer, layer.revision, current)?.unwrap_or_default())
}

/// Layer `layer_id` ("layer-12") as it was at `revision`: its file, the
/// version and surface it was drawn on, its drawing then, and whether the
/// file has changed since (`outdated`, as of now). None when there's no
/// such layer, or that revision isn't kept. For review items
/// (`crate::annotations::ink`).
#[allow(dead_code)] // Review items (slice C) build on it.
pub(crate) fn at(db: &Connection, layer_id: &str, revision: u32) -> rusqlite::Result<Option<v1::InkLayer>> {
    let Some(layer) = find(db, layer_id)? else { return Ok(None) };
    layer_at(db, &layer, revision, &current_sha256(Path::new(&layer.path)))
}

/// Every layer on a file, each version's, newest first, each at its latest
/// revision; `current` is the file's SHA-256 now.
fn list(db: &Connection, path: &str, current: String) -> rusqlite::Result<v1::InkLayerList> {
    let layers: Vec<Layer> =
        crate::store::query(db, &format!("SELECT {LAYER_COLUMNS} FROM ink_layers WHERE path = ?1 ORDER BY id DESC"), [path], read_layer)?;
    let layers = layers.iter().map(|layer| latest(db, layer, &current)).collect::<rusqlite::Result<_>>()?;
    Ok(v1::InkLayerList { path: path.to_owned(), current_sha256: current, layers })
}

/// Whether a kept revision names blob `sha` (its drawing or preview), for
/// `crate::ink::referenced`.
pub(crate) fn refers_to(db: &Connection, sha: &str) -> rusqlite::Result<bool> {
    db.query_row(
        "SELECT EXISTS (SELECT 1 FROM ink_revisions WHERE drawing_sha256 = ?1)
             OR EXISTS (SELECT 1 FROM ink_revisions WHERE preview_sha256 = ?1)",
        [sha],
        |row| row.get(0),
    )
}

// ---- Saving ----

/// Who saves: the device's id and kind.
pub(crate) struct Saver<'a> {
    pub device_id: &'a str,
    pub device_kind: &'a str,
}

/// What a save comes to.
enum Saved {
    Revision { layer: i64, revision: u32, freed: Vec<String> },
    Repeat { layer: i64, revision: u32 },
    Conflict(Box<v1::InkLayer>),
}

/// The checked blobs a save names: the drawing's size, and the preview's
/// size and pixels.
struct Blobs {
    drawing_bytes: u64,
    preview_bytes: u64,
    preview_width: u32,
    preview_height: u32,
}

impl Factory {
    /// The layers on `path` again, to every client.
    fn broadcast_ink_layers(&self, path: &str, current: String) -> rusqlite::Result<()> {
        let list = list(&self.store.db(), path, current)?;
        self.send(Body::InkLayerList(list));
        Ok(())
    }

    /// ListInkLayers.
    pub(crate) fn list_ink_layers(&self, request: &v1::ListInkLayers) -> Result<v1::InkLayerList, RequestError> {
        let path = absolute(&request.path)?;
        // Hashed before the records are held: a big file takes a while once.
        let current = current_sha256(Path::new(&path));
        Ok(list(&self.store.db(), &path, current)?)
    }

    /// The blobs a save names, whole in the store and within their limits.
    fn checked_blobs(&self, request: &v1::SaveInkLayer) -> Result<Blobs, RequestError> {
        let drawing = super::drawing(&self.home, &request.drawing_sha256).map_err(refused)?;
        let (preview, preview_width, preview_height) =
            super::picture(&self.home, &request.preview_sha256, super::PREVIEW_MAX, None).map_err(refused)?;
        if drawing.bytes == 0 {
            return Err(refused("The drawing is empty; save it again."));
        }
        Ok(Blobs { drawing_bytes: drawing.bytes, preview_bytes: preview.bytes, preview_width, preview_height })
    }

    /// SaveInkLayer: answered only once the revision is durable.
    pub(crate) fn save_ink_layer(
        self: &Arc<Self>,
        request: &v1::SaveInkLayer,
        saver: &Saver<'_>,
    ) -> Result<v1::InkLayerSaved, RequestError> {
        let save_id = request.client_save_id.trim();
        if save_id.is_empty() || save_id.len() > 128 {
            return Err(refused("A save needs its client_save_id, up to 128 characters."));
        }
        let answer = |layer: i64, revision: u32| v1::InkLayerSaved {
            client_save_id: save_id.to_owned(),
            ink_layer_id: layer_name(layer),
            revision,
            conflict: None,
        };
        // A retry: what it answered.
        let earlier = |db: &Connection| {
            db.query_row(
                "SELECT layer, revision FROM ink_saves WHERE device_id = ?1 AND client_save_id = ?2",
                params![saver.device_id, save_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, u32>(1)?)),
            )
            .optional()
        };
        if let Some((layer, revision)) = earlier(&self.store.db())? {
            return Ok(answer(layer, revision));
        }
        let path = absolute(&request.path)?;
        let space = request.space.clone().ok_or_else(|| refused("A save needs its surface."))?;
        let surface = surface(&space)?;
        if !fits(&space, &request.drawing_space) {
            return Err(refused("The drawing's space doesn't fit its surface."));
        }
        if !finite(&request.drawing_to_document, 6) {
            return Err(refused("drawing_to_document is six numbers."));
        }
        if request.content_version == 0 {
            return Err(refused("A drawing needs its content_version."));
        }
        if request.os_version.len() > 32 || request.conflict_of.len() > 32 || request.ink_layer_id.len() > 32 {
            return Err(refused("os_version, conflict_of or ink_layer_id is too long."));
        }
        let creating = request.ink_layer_id.is_empty();
        if creating && request.base_revision != 0 {
            return Err(refused("A new layer has no revision to save over."));
        }
        if !creating && !request.conflict_of.is_empty() {
            return Err(refused("Only a new layer is kept beside another."));
        }
        // The file as it is now, hashed before the records are held. A new
        // layer is drawn on it; a revision is kept whatever it is now.
        let now = source(Path::new(&path));
        if creating && request.conflict_of.is_empty() {
            let now = now.as_ref().map_err(|why| refused(why.clone()))?;
            if now.sha256 != request.source_sha256 {
                return Err(refused("This file changed since you opened it."));
            }
        }
        let current = now.as_ref().map(|now| now.sha256.clone()).unwrap_or_default();

        let mut db = self.store.db();
        // Durable before it's said to be saved: this commit is synced.
        db.pragma_update(None, "synchronous", "FULL")?;
        let saved = (|| -> Result<Saved, RequestError> {
            let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if let Some((layer, revision)) = earlier(&transaction)? {
                return Ok(Saved::Repeat { layer, revision });
            }
            // Checked with the records held, so the blob sweep can't take
            // them away before this revision names them.
            let blobs = self.checked_blobs(request)?;
            let now_ms = now_ms();
            let conflict =
                |layer: &Layer| -> Result<Saved, RequestError> { Ok(Saved::Conflict(Box::new(latest(&transaction, layer, &current)?))) };
            let (layer, revision) = if creating {
                let (sha, mime, size) = if request.conflict_of.is_empty() {
                    let taken = transaction
                        .query_row(
                            &format!(
                                "SELECT {LAYER_COLUMNS} FROM ink_layers WHERE path = ?1 AND source_sha256 = ?2 AND surface = ?3 AND conflict_of = ''"
                            ),
                            params![path, request.source_sha256, surface],
                            read_layer,
                        )
                        .optional()?;
                    // Another device made this surface's layer first.
                    if let Some(layer) = taken {
                        return conflict(&layer);
                    }
                    let now = now.as_ref().map_err(|why| refused(why.clone()))?;
                    (now.sha256.clone(), now.mime.to_owned(), now.size)
                } else {
                    let beside = find(&transaction, &request.conflict_of)?
                        .filter(|layer| layer.path == path && layer.surface == surface)
                        .ok_or_else(|| refused("conflict_of isn't a layer on this surface."))?;
                    if beside.source_sha256 != request.source_sha256 {
                        return Err(refused("This file changed since you opened it."));
                    }
                    (beside.source_sha256, beside.source_mime, beside.source_size)
                };
                transaction.execute(
                    "INSERT INTO ink_layers (path, source_sha256, source_mime, source_size, surface, space, conflict_of, revision, created)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8)",
                    params![path, sha, mime, size as i64, surface, space.encode_to_vec(), request.conflict_of, now_ms],
                )?;
                (transaction.last_insert_rowid(), 1)
            } else {
                let layer = find(&transaction, &request.ink_layer_id)?
                    .ok_or_else(|| refused(format!("There's no ink layer \"{}\"; it may have been deleted.", request.ink_layer_id)))?;
                if layer.path != path || layer.surface != surface || layer.source_sha256 != request.source_sha256 {
                    return Err(refused("This save isn't for that layer's file and surface."));
                }
                if request.base_revision != layer.revision {
                    return conflict(&layer);
                }
                let next = layer.revision + 1;
                transaction.execute("UPDATE ink_layers SET revision = ?2 WHERE id = ?1", params![layer.id, next])?;
                transaction.execute(
                    "UPDATE ink_revisions SET superseded = ?3 WHERE layer = ?1 AND revision = ?2",
                    params![layer.id, layer.revision, now_ms],
                )?;
                (layer.id, next)
            };
            transaction.execute(
                "INSERT INTO ink_revisions (layer, revision, parent_revision, drawing_space, drawing_to_document, content_version,
                     drawing_sha256, drawing_bytes, preview_sha256, preview_bytes, preview_width, preview_height,
                     device_id, device_kind, os_version, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    layer,
                    revision,
                    revision - 1,
                    request.drawing_space,
                    serde_json::to_string(&request.drawing_to_document)?,
                    request.content_version,
                    request.drawing_sha256,
                    blobs.drawing_bytes as i64,
                    request.preview_sha256,
                    blobs.preview_bytes as i64,
                    blobs.preview_width,
                    blobs.preview_height,
                    saver.device_id,
                    saver.device_kind,
                    request.os_version,
                    now_ms
                ],
            )?;
            transaction.execute(
                "INSERT INTO ink_saves (device_id, client_save_id, layer, revision, created) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![saver.device_id, save_id, layer, revision, now_ms],
            )?;
            let freed = prune(&transaction, now_ms)?;
            transaction.commit()?;
            Ok(Saved::Revision { layer, revision, freed })
        })();
        if let Err(error) = db.pragma_update(None, "synchronous", "NORMAL") {
            tracing::warn!(%error, "couldn't set factory.db's synchronous back to NORMAL");
        }
        match saved? {
            Saved::Repeat { layer, revision } => Ok(answer(layer, revision)),
            Saved::Conflict(layer) => Ok(v1::InkLayerSaved {
                client_save_id: save_id.to_owned(),
                ink_layer_id: layer.id.clone(),
                revision: 0,
                conflict: Some(*layer),
            }),
            Saved::Revision { layer, revision, freed } => {
                drop_blobs(&self.home, &db, &freed);
                drop(db);
                self.broadcast_ink_layers(&path, current)?;
                self.sweep_ink_blobs();
                Ok(answer(layer, revision))
            }
        }
    }

    /// DeleteInkLayer: at the revision the person saw, and never while a
    /// review item refers to it. Its revisions go with it.
    pub(crate) fn delete_ink_layer(self: &Arc<Self>, request: &v1::DeleteInkLayer) -> Result<(), RequestError> {
        let mut db = self.store.db();
        let transaction = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let layer = find(&transaction, &request.ink_layer_id)?
            .ok_or_else(|| refused(format!("There's no ink layer \"{}\"; it may have been deleted already.", request.ink_layer_id)))?;
        if request.base_revision != layer.revision {
            return Err(refused("This ink changed on another device. Look at it again, then delete it."));
        }
        if in_use(&transaction, &layer_name(layer.id)) {
            return Err(refused("This ink is part of a comment. Delete the comment first."));
        }
        let freed = blobs_of(&transaction, "layer = ?1", params![layer.id])?;
        transaction.execute("DELETE FROM ink_revisions WHERE layer = ?1", [layer.id])?;
        transaction.execute("DELETE FROM ink_saves WHERE layer = ?1", [layer.id])?;
        transaction.execute("DELETE FROM ink_layers WHERE id = ?1", [layer.id])?;
        transaction.commit()?;
        drop_blobs(&self.home, &db, &freed);
        drop(db);
        self.broadcast_ink_layers(&layer.path, current_sha256(Path::new(&layer.path)))?;
        self.sweep_ink_blobs();
        Ok(())
    }

    /// Sweeps the blob store for blobs nothing refers to, at most once a
    /// day, in the background.
    fn sweep_ink_blobs(self: &Arc<Self>) {
        static SWEPT: Mutex<BTreeMap<PathBuf, i64>> = Mutex::new(BTreeMap::new());
        let now = now_ms();
        {
            let mut swept = SWEPT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let last = swept.entry(self.home.root().to_owned()).or_insert(i64::MIN);
            if now.saturating_sub(*last) < SWEEP_EVERY_MS {
                return;
            }
            *last = now;
        }
        let factory = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let Ok(entries) = std::fs::read_dir(super::root(&factory.home).join("blobs")) else { return };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(sha) = name.to_str().filter(|name| super::valid_sha(name)) else { continue };
                drop_blobs(&factory.home, &factory.store.db(), &[sha.to_owned()]);
            }
        });
    }
}

/// The blobs the revisions `filter` picks name.
fn blobs_of(db: &Connection, filter: &str, values: impl rusqlite::Params) -> rusqlite::Result<Vec<String>> {
    let mut statement = db.prepare(&format!("SELECT drawing_sha256, preview_sha256 FROM ink_revisions WHERE {filter}"))?;
    let pairs = statement.query_map(values, |row| Ok([row.get::<_, String>(0)?, row.get::<_, String>(1)?]))?;
    let mut shas = BTreeSet::new();
    for pair in pairs {
        shas.extend(pair?);
    }
    Ok(shas.into_iter().collect())
}

/// Removes the blobs among `shas` nothing refers to any more and that are
/// over a day old. `db` is held meanwhile, so no save can come to name one
/// as it goes: a save checks its blobs with the records held.
fn drop_blobs(home: &crate::home::Home, db: &Connection, shas: &[String]) {
    let now = SystemTime::now();
    for sha in shas {
        // Kept when the records can't be read.
        if super::referenced(db, sha).unwrap_or(true) {
            continue;
        }
        let Some(blob) = super::blob(home, sha) else { continue };
        let old = std::fs::metadata(&blob.path)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age >= BLOB_KEPT));
        if old && let Err(error) = std::fs::remove_file(&blob.path) {
            tracing::warn!(%error, sha, "couldn't remove an ink blob nothing refers to");
        }
    }
}

/// Deletes revisions saved over a day ago that no review item keeps, and
/// what saves answered a week ago: the blobs those revisions named.
fn prune(transaction: &Transaction<'_>, now: i64) -> rusqlite::Result<Vec<String>> {
    let old: Vec<(i64, u32)> = {
        let mut statement =
            transaction.prepare("SELECT layer, revision FROM ink_revisions WHERE superseded IS NOT NULL AND superseded < ?1")?;
        statement.query_map([now - SUPERSEDED_KEPT_MS], |row| Ok((row.get(0)?, row.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    let mut freed = BTreeSet::new();
    for (layer, revision) in old {
        if !item_revisions(transaction, &layer_name(layer))?.contains(&revision) {
            freed.extend(blobs_of(transaction, "layer = ?1 AND revision = ?2", params![layer, revision])?);
            transaction.execute("DELETE FROM ink_revisions WHERE layer = ?1 AND revision = ?2", params![layer, revision])?;
        }
    }
    transaction.execute("DELETE FROM ink_saves WHERE created < ?1", [now - SAVES_KEPT_MS])?;
    Ok(freed.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pdf_page(page: u32) -> v1::InkSpace {
        v1::InkSpace {
            kind: "pdf_page".into(),
            page_1based: page,
            r#box: "crop".into(),
            crop_box: vec![0.0, 0.0, 612.0, 792.0],
            media_box: vec![0.0, 0.0, 612.0, 792.0],
            rotation: 0,
            width: 612.0,
            height: 792.0,
            units: "pt".into(),
            ..v1::InkSpace::default()
        }
    }

    #[test]
    fn surfaces_are_checked_and_named() {
        assert_eq!(surface(&pdf_page(3)).ok().as_deref(), Some("page:3"));
        for broken in [
            pdf_page(0),
            pdf_page(2001),
            v1::InkSpace { rotation: 45, ..pdf_page(1) },
            v1::InkSpace { crop_box: vec![0.0, 0.0, f64::NAN, 1.0], ..pdf_page(1) },
            v1::InkSpace { media_box: vec![0.0, 0.0, 1.0], ..pdf_page(1) },
            v1::InkSpace { units: "px".into(), ..pdf_page(1) },
            v1::InkSpace { width: f64::INFINITY, ..pdf_page(1) },
            v1::InkSpace { r#box: "media".into(), ..pdf_page(1) },
        ] {
            assert!(surface(&broken).is_err(), "{broken:?}");
        }
        let image = v1::InkSpace {
            kind: "image".into(),
            oriented_width: 3024,
            oriented_height: 4032,
            stored_width: 4032,
            stored_height: 3024,
            exif_orientation: 6,
            units: "px".into(),
            ..v1::InkSpace::default()
        };
        assert_eq!(surface(&image).ok().as_deref(), Some("image"));
        assert!(surface(&v1::InkSpace { exif_orientation: 9, ..image.clone() }).is_err());
        assert!(surface(&v1::InkSpace { stored_width: 0, ..image.clone() }).is_err());
        assert!(surface(&v1::InkSpace { oriented_height: 70_000, ..image.clone() }).is_err());
        let excerpt = v1::InkSpace {
            kind: "excerpt_rendition".into(),
            width: 640.0,
            height: 178.0,
            units: "pt".into(),
            text: "Then by the chain rule".into(),
            rendition: Some(v1::ExcerptRendition { font: "Geist".into(), font_size: 17.0, line_height: 1.5, padding: 24.0 }),
            ..v1::InkSpace::default()
        };
        let named = surface(&excerpt).expect("an excerpt");
        assert!(named.starts_with("excerpt:") && named.len() == "excerpt:".len() + 64);
        assert_ne!(
            surface(&v1::InkSpace { text: "Another".into(), ..excerpt.clone() }).ok(),
            Some(named),
            "each excerpt is its own surface"
        );
        assert!(surface(&v1::InkSpace { text: "x".repeat(64 * 1024 + 1), ..excerpt.clone() }).is_err());
        assert!(surface(&v1::InkSpace { rendition: None, ..excerpt.clone() }).is_err());
        assert!(surface(&v1::InkSpace { kind: "sketch".into(), ..excerpt }).is_err());
        assert!(fits(&pdf_page(1), "page_overlay") && !fits(&pdf_page(1), "oriented_image"));
        assert!(fits(&image, "blank_canvas") && fits(&image, "oriented_image") && !fits(&image, "excerpt_rendition"));
    }

    #[test]
    fn names_and_kinds() {
        assert_eq!(layer_number(&layer_name(12)), Some(12));
        assert_eq!(layer_number("layer-0"), None);
        assert_eq!(layer_number("layer-x"), None);
        assert_eq!(layer_number("12"), None);
        assert_eq!(mime(b"%PDF-1.7\n", Path::new("/a")), "application/pdf");
        assert_eq!(mime(b"\0\0\0\x18ftypheic", Path::new("/a.heic")), "image/heic");
        assert_eq!(mime(b"# Notes", Path::new("/a.md")), "text/markdown");
        assert_eq!(mime(b"notes", Path::new("/a.txt")), "text/plain");
        assert_eq!(mime(b"\xff\xfe\0\x01", Path::new("/a.bin")), "application/octet-stream");
    }
}
