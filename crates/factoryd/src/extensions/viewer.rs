//! The Viewer extension: agents show the person files in the app (`show`),
//! instead of opening apps on Factory's machine, which may not be the one
//! the person is at.
//!
//! An agent's main area is a layout of panes (`Agent.layout`): its
//! conversation, which every layout has once, and viewers beside it, split
//! in rows and columns. The person arranges it in the app (SetLayout); `show`
//! adds a viewer to it. Layouts are kept in factory.db, the same on every
//! device. Viewers read their files through the protocol (ReadFile), since
//! the app may be on another machine.

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::{self, server_message::Body, view_node::Node};
use prost::Message as _;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Extension, Manifest, PromptSection, agent_handler, live, tool};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner, RequestError};
use crate::store::Store;

pub(crate) const ID: &str = "viewer";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Viewer",
    icon: "panels-top-left",
    description: "Lets it show you files, such as text, images and videos, beside its conversation.",
    default_enabled: true,
    options: &[],
};

const SHOW: &str = "Show the person a file in the app, in a pane beside your conversation: text (code, Markdown, logs, CSV…), an image, a video (MP4 or QuickTime) or a PDF. Use it instead of opening apps such as Preview or a browser on this machine: the person may be using Claktory from another device. It doesn't send them the file: a report or file they asked to be sent, shown or notified of goes as ask_person with review, which waits in their Needs you; show only adds it beside your conversation. path: absolute, or relative to your working folder. kind: \"text\", \"image\", \"video\" or \"pdf\", inferred when left out. A file already open isn't opened twice; showing it again counts as opening it now. The person sets how many panes can be open beside your conversation (Settings → Display): past that, show replaces the pane opened longest ago (never the one they expanded to fill the window), and its result says which. Returns at once.";

pub(crate) const SCHEMA: &str = "
-- How each agent's main area is laid out (the Viewer extension): its
-- conversation and the viewers beside it, as an encoded ViewNode.
CREATE TABLE layouts (agent TEXT PRIMARY KEY, layout BLOB NOT NULL);
";

/// The migration that added the pane shown in focus: a leaf's id
/// in the layout, '' for none.
pub(crate) const FOCUS: &str = "
ALTER TABLE layouts ADD COLUMN focus TEXT NOT NULL DEFAULT '';
";

/// The migration that added when each pane was opened, so a full layout's
/// show replaces the oldest: a JSON object of pane id to ms; null before
/// it, when every pane reads as opened before any other (layout order
/// between them).
pub(crate) const OPENED: &str = "
ALTER TABLE layouts ADD COLUMN opened TEXT;
";

/// Text is read up to this much, whatever is asked…
const TEXT_LIMIT: u64 = 2_000_000;
/// …and images up to this much.
const IMAGE_LIMIT: u64 = 20_000_000;
/// Videos, and any file read from an offset, are read this much at a time…
const RANGE_LIMIT: u64 = 8 * 1024 * 1024;
/// …from files up to this size, which are hashed whole for every range.
const VIDEO_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
/// PDFs are read whole a range at a time, up to this size (iPad phase 3 §2).
const PDF_LIMIT: u64 = 64 * 1024 * 1024;
/// Files whose hash is kept (`whole_sha256`).
const HASHES_KEPT: usize = 64;
/// Panes in one layout, the conversation included.
const MAX_LEAVES: usize = 12;
// The show tool opens panes up to the person's setting
// (`DisplayConfig::open_panes`), which is at most a layout's other panes.
const _: () = assert!(crate::config::MAX_OPEN_PANES as usize == MAX_LEAVES - 1);
/// Splits within splits.
const MAX_DEPTH: usize = 8;

const CHAT: &str = "chat";
const KINDS: [&str; 4] = ["text", "image", "video", "pdf"];

/// Agents' layouts, as kept in factory.db.
#[derive(Default)]
pub(crate) struct State {
    layouts: Mutex<HashMap<String, Kept>>,
}

/// An agent's layout, the pane shown in focus ('' for none), and when each
/// pane was opened (ms; 0 from before it was kept).
#[derive(Clone)]
struct Kept {
    root: v1::ViewNode,
    focus: String,
    opened: HashMap<String, i64>,
}

impl State {
    /// The layouts in factory.db; those that can't be read are left out.
    pub(crate) fn load(store: &Store) -> State {
        let mut layouts = HashMap::new();
        /// Agent, encoded layout, focus, and when its panes were opened.
        type Row = (String, Vec<u8>, String, Option<String>);
        let read = || -> rusqlite::Result<Vec<Row>> {
            let db = store.db();
            let mut statement = db.prepare("SELECT agent, layout, focus, opened FROM layouts")?;
            let rows = statement.query_map([], crate::store::row4)?;
            rows.collect()
        };
        match read() {
            Ok(rows) => {
                for (agent, bytes, focus, opened) in rows {
                    match v1::ViewNode::decode(bytes.as_slice()) {
                        Ok(root) => {
                            // A focus that names no pane is none.
                            let focus = if leaves(&root).iter().any(|(id, _)| *id == focus) { focus } else { String::new() };
                            let opened = opened.and_then(|opened| serde_json::from_str(&opened).ok()).unwrap_or_default();
                            let opened = opened_now(&root, &opened, 0);
                            layouts.insert(agent, Kept { root, focus, opened });
                        }
                        Err(error) => tracing::warn!(%agent, %error, "an agent's layout can't be read; it shows just its conversation"),
                    }
                }
            }
            Err(error) => tracing::error!(%error, "could not read the agents' layouts"),
        }
        State { layouts: Mutex::new(layouts) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Kept>> {
        self.layouts.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The agent's layout, each pane with when it was opened; None for just
    /// its conversation.
    pub(crate) fn layout(&self, agent: &str) -> Option<v1::ViewNode> {
        self.lock().get(agent).map(|kept| {
            let mut root = kept.root.clone();
            stamp(&mut root, &kept.opened);
            root
        })
    }

    /// The id of the agent's pane shown in focus; empty for none.
    pub(crate) fn focus(&self, agent: &str) -> String {
        self.lock().get(agent).map(|kept| kept.focus.clone()).unwrap_or_default()
    }

    /// The paths of every agent's panes of this kind, each once.
    pub(crate) fn paths(&self, kind: &str) -> HashSet<String> {
        let layouts = self.lock();
        let all = layouts.values().flat_map(|kept| leaves(&kept.root));
        all.filter(|(_, leaf)| leaf.kind == kind && !leaf.path.is_empty()).map(|(_, leaf)| leaf.path.clone()).collect()
    }
}

/// `path` with "~" for the home folder.
pub(crate) fn expand(path: &str) -> PathBuf {
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    match path {
        "~" => home,
        _ => path.strip_prefix("~/").map_or_else(|| PathBuf::from(path), |rest| home.join(rest)),
    }
}

/// An I/O error as people read it: "permission denied", not "Permission
/// denied (os error 13)".
fn plain(error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::NotFound => "no such file".to_owned(),
        std::io::ErrorKind::PermissionDenied => "permission denied".to_owned(),
        _ => {
            let text = error.to_string();
            let text = text.split(" (os error").next().unwrap_or(&text).to_owned();
            let mut chars = text.chars();
            chars.next().map(|first| first.to_lowercase().chain(chars).collect()).unwrap_or(text)
        }
    }
}

/// What the file is, from its first bytes and its name.
fn sniff(start: &[u8], path: &Path) -> &'static str {
    let magic: [(&[u8], &str); 6] = [
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF8", "image/gif"),
        (b"BM", "image/bmp"),
        (b"%PDF-", "application/pdf"),
        (b"\0\0\0", ""),
    ];
    if start.len() >= 12 && &start[..4] == b"RIFF" && &start[8..12] == b"WEBP" {
        return "image/webp";
    }
    if let Some(mime) = sniff_movie(start, path) {
        return mime;
    }
    for (prefix, mime) in magic {
        if !mime.is_empty() && start.starts_with(prefix) {
            return mime;
        }
    }
    if !is_text(start) {
        return "application/octet-stream";
    }
    let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or_default().to_ascii_lowercase();
    match extension.as_str() {
        "md" | "markdown" => "text/markdown",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "svg" => "image/svg+xml",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "js" | "mjs" => "text/javascript",
        _ => "text/plain",
    }
}

/// An MP4 or QuickTime movie, from its first box (an "ftyp" box's brand,
/// or the extension for older movies that start without one), or a WebM.
fn sniff_movie(start: &[u8], path: &Path) -> Option<&'static str> {
    if start.starts_with(b"\x1a\x45\xdf\xa3") {
        return Some("video/webm");
    }
    let extension = path.extension().and_then(|extension| extension.to_str()).unwrap_or_default().to_ascii_lowercase();
    let by_extension = match extension.as_str() {
        "mov" | "qt" => Some("video/quicktime"),
        "mp4" | "m4v" => Some("video/mp4"),
        _ => None,
    };
    let first = start.get(4..8)?;
    if first != b"ftyp" {
        return matches!(first, b"moov" | b"mdat" | b"wide" | b"free" | b"skip" | b"pnot").then_some(by_extension).flatten();
    }
    match start.get(8..12)? {
        b"qt  " => Some("video/quicktime"),
        // Still images (HEIF, AVIF) and sound (M4A, audiobooks) in the same boxes.
        b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"hevm" | b"hevs" | b"mif1" | b"msf1" | b"avif" | b"avis" => None,
        b"M4A " | b"M4B " | b"M4P " | b"F4A " | b"F4B " => None,
        _ => Some(by_extension.unwrap_or("video/mp4")),
    }
}

/// Videos the app plays (Apple's players don't play WebM).
fn is_video(mime: &str) -> bool {
    matches!(mime, "video/mp4" | "video/quicktime")
}

fn is_pdf(mime: &str) -> bool {
    mime == "application/pdf"
}

/// UTF-8 without NULs; a character cut at the end of what was read counts.
fn is_text(bytes: &[u8]) -> bool {
    if bytes.contains(&0) {
        return false;
    }
    match std::str::from_utf8(bytes) {
        Ok(_) => true,
        Err(error) => error.error_len().is_none() && bytes.len() - error.valid_up_to() < 4,
    }
}

/// Raster images the app shows; other image types (SVG) show as text.
fn is_image(mime: &str) -> bool {
    matches!(mime, "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp")
}

/// Reads up to `max_bytes` of a file (0: the default) from `offset`, for a
/// viewer: text and images from the start as they are (up to their limits,
/// hashed when whole), videos and other files read from an offset a range
/// at a time, with the whole file's hash (`whole_sha256`).
pub(crate) fn read_file(path: &str, offset: u64, max_bytes: u64) -> v1::FileContent {
    read(path, offset, max_bytes, true)
}

/// ReadFile: `read_file`, from the version `expected_sha256` names when it's
/// set, or with `error` "changed" when the file is no longer that version.
pub(crate) fn read_requested(request: &v1::ReadFile) -> v1::FileContent {
    let expected = request.expected_sha256.trim();
    let content = read(&request.path, request.offset, request.max_bytes, true);
    if expected.is_empty() || !content.error.is_empty() {
        return content;
    }
    // A text or image read whole has the hash of what it read; a range has
    // the whole file's.
    let now = if content.sha256.is_empty() {
        match whole_sha256(Path::new(&content.path)) {
            Ok(sha) => sha,
            Err(error) => {
                let error = format!("Can't read {}: {}", content.path, plain(&error));
                return v1::FileContent { path: content.path, error, ..v1::FileContent::default() };
            }
        }
    } else {
        content.sha256.clone()
    };
    if !now.eq_ignore_ascii_case(expected) {
        return v1::FileContent { path: content.path, error: "changed".to_owned(), ..v1::FileContent::default() };
    }
    content
}

/// `read_file`, hashing ranges of big files only when `hash`.
fn read(path: &str, offset: u64, max_bytes: u64, hash: bool) -> v1::FileContent {
    let path = expand(path.trim());
    read_named(&path, &path, offset, max_bytes, hash)
}

/// A kept copy of the file `name` (a sealed version), read as `read_file`
/// reads `name`: its type from its bytes and `name`, and `name` as its path.
pub(crate) fn read_copy(copy: &Path, name: &Path, offset: u64, max_bytes: u64) -> v1::FileContent {
    read_named(copy, name, offset, max_bytes, true)
}

/// Reads `path`, as the file `name` (its type, its path in the answer and
/// in errors).
fn read_named(path: &Path, name: &Path, offset: u64, max_bytes: u64, hash: bool) -> v1::FileContent {
    let mut content = v1::FileContent { path: name.display().to_string(), ..v1::FileContent::default() };
    let failed = |mut content: v1::FileContent, error: String| {
        content.error = error;
        content
    };
    if !path.is_absolute() {
        return failed(content, format!("{} isn't a full path: start it with / or ~", name.display()));
    }
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return failed(content, format!("No such file: {}", name.display())),
        Err(error) => return failed(content, format!("Can't read {}: {}", name.display(), plain(&error))),
    };
    if metadata.is_dir() {
        return failed(content, format!("{} is a folder", name.display()));
    }
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => return failed(content, format!("Can't read {}: {}", name.display(), plain(&error))),
    };
    let mut start = Vec::new();
    if let Err(error) = (&mut file).take(512).read_to_end(&mut start) {
        return failed(content, format!("Can't read {}: {}", name.display(), plain(&error)));
    }
    let mime = sniff(&start, name);
    let ranged = offset > 0 || is_video(mime) || is_pdf(mime);
    if is_pdf(mime) && metadata.len() > PDF_LIMIT {
        let megabytes = metadata.len().div_ceil(1_000_000);
        return failed(content, format!("This PDF is too big to open here ({megabytes} MB)."));
    }
    if ranged && metadata.len() > VIDEO_LIMIT {
        return failed(content, format!("{} is too big to play. Claktory plays videos up to 2 GB.", name.display()));
    }
    let limit = if ranged {
        RANGE_LIMIT
    } else if is_image(mime) {
        IMAGE_LIMIT
    } else {
        TEXT_LIMIT
    };
    let limit = if max_bytes == 0 { limit } else { max_bytes.min(limit) };
    let mut data = if offset == 0 { start } else { Vec::new() };
    data.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    if offset > 0
        && let Err(error) = std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(offset))
    {
        return failed(content, format!("Can't read {}: {}", name.display(), plain(&error)));
    }
    let rest = limit.saturating_sub(data.len() as u64);
    if let Err(error) = file.take(rest).read_to_end(&mut data) {
        return failed(content, format!("Can't read {}: {}", name.display(), plain(&error)));
    }
    content.mime = mime.to_owned();
    content.size = metadata.len();
    content.offset = offset;
    content.truncated = offset > 0 || (data.len() as u64) < metadata.len();
    if !content.truncated {
        content.sha256 = crate::encoding::hex(&<sha2::Sha256 as sha2::Digest>::digest(&data));
    } else if ranged && hash {
        match whole_sha256(path) {
            Ok(sha) => content.sha256 = sha,
            Err(error) => return failed(content, format!("Can't read {}: {}", name.display(), plain(&error))),
        }
    }
    content.data = data;
    content.modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX));
    content
}

/// A file as last hashed: its size and when it was changed.
type Stamp = (u64, Option<std::time::SystemTime>);

/// Whole files' hashes (`whole_sha256`), by path, with the stamp they were
/// made at.
static HASHES: std::sync::LazyLock<Mutex<HashMap<PathBuf, (Stamp, String)>>> = std::sync::LazyLock::new(Mutex::default);

fn stamp_of(metadata: &std::fs::Metadata) -> Stamp {
    (metadata.len(), metadata.modified().ok())
}

/// The whole file's SHA-256, in lowercase hex, as FileContent.sha256 has it:
/// kept by path, size and modification time, so a video read a range at a
/// time (or checked every second while it's shown) is hashed once per
/// version. Files over 2 GB aren't hashed: that's InvalidInput.
pub(crate) fn whole_sha256(path: &Path) -> std::io::Result<String> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > VIDEO_LIMIT {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "over 2 GB"));
    }
    let stamp = stamp_of(&metadata);
    let lock = || HASHES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((kept, sha)) = lock().get(path)
        && *kept == stamp
    {
        return Ok(sha.clone());
    }
    let mut file = std::fs::File::open(path)?;
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sha2::Digest::update(&mut hasher, &buffer[..read]);
    }
    let sha = crate::encoding::hex(&sha2::Digest::finalize(hasher));
    // Kept only if it didn't change while it was read.
    if std::fs::metadata(path).is_ok_and(|after| stamp_of(&after) == stamp) {
        let mut hashes = lock();
        if hashes.len() >= HASHES_KEPT
            && !hashes.contains_key(path)
            && let Some(gone) = hashes.keys().next().cloned()
        {
            hashes.remove(&gone);
        }
        hashes.insert(path.to_owned(), (stamp, sha.clone()));
    }
    Ok(sha)
}

fn leaf_of(node: &v1::ViewNode) -> Option<&v1::ViewLeaf> {
    match &node.node {
        Some(Node::Leaf(leaf)) => Some(leaf),
        _ => None,
    }
}

/// Every leaf, depth first.
fn leaves(node: &v1::ViewNode) -> Vec<(&str, &v1::ViewLeaf)> {
    match &node.node {
        Some(Node::Leaf(leaf)) => vec![(node.id.as_str(), leaf)],
        Some(Node::Split(split)) => split.children.iter().flat_map(leaves).collect(),
        None => Vec::new(),
    }
}

/// Why a layout can't be used, if it can't.
pub(crate) fn validate(root: &v1::ViewNode) -> Result<(), String> {
    fn walk<'a>(node: &'a v1::ViewNode, depth: usize, ids: &mut HashSet<&'a str>) -> Result<(), String> {
        if node.id.trim().is_empty() {
            return Err("Every pane and split needs an id.".to_owned());
        }
        if !ids.insert(node.id.as_str()) {
            return Err(format!("Two panes or splits have the id \"{}\".", node.id));
        }
        if depth > MAX_DEPTH {
            return Err(format!("Splits go at most {MAX_DEPTH} deep."));
        }
        match &node.node {
            None => Err(format!("\"{}\" is neither a split nor a pane.", node.id)),
            Some(Node::Leaf(leaf)) if leaf.kind.trim().is_empty() => Err(format!("The pane \"{}\" has no kind.", node.id)),
            Some(Node::Leaf(_)) => Ok(()),
            Some(Node::Split(split)) => {
                if !matches!(split.axis(), v1::ViewAxis::Row | v1::ViewAxis::Column) {
                    return Err(format!("The split \"{}\" needs an axis, row or column.", node.id));
                }
                if split.children.len() < 2 {
                    return Err(format!("The split \"{}\" needs two or more panes.", node.id));
                }
                if !split.weights.is_empty()
                    && (split.weights.len() != split.children.len()
                        || split.weights.iter().any(|weight| !weight.is_finite() || *weight <= 0.0))
                {
                    return Err(format!("The split \"{}\" needs a positive weight for each pane, or none.", node.id));
                }
                split.children.iter().try_for_each(|child| walk(child, depth + 1, ids))
            }
        }
    }
    walk(root, 0, &mut HashSet::new())?;
    let all = leaves(root);
    let chats = all.iter().filter(|(_, leaf)| leaf.kind == CHAT).count();
    if chats != 1 {
        return Err(format!("A layout has the conversation (a \"chat\" pane) exactly once; this one has it {chats} times."));
    }
    if all.len() > MAX_LEAVES {
        return Err(format!("A layout has at most {MAX_LEAVES} panes; this one has {}.", all.len()));
    }
    Ok(())
}

/// Just the conversation.
fn is_plain(root: &v1::ViewNode) -> bool {
    leaf_of(root).is_some_and(|leaf| leaf.kind == CHAT)
}

/// Saves the agent's layout (None: just its conversation) with the pane
/// in focus (a leaf of it, or empty), and shows it on every device. Just
/// the conversation has nothing to focus.
fn store_layout(factory: &Factory, agent: &str, root: Option<v1::ViewNode>, focus: &str) -> Result<(), RequestError> {
    let mut root = root.filter(|root| !is_plain(root));
    // When each pane was opened is kept beside the layout, not in it: what
    // a device sends back of it is left out.
    if let Some(root) = &mut root {
        stamp(root, &HashMap::new());
    }
    let focus = if root.is_some() { focus } else { "" };
    // Panes it had keep when they were opened; new ones are opened now.
    let opened = root.as_ref().map(|root| {
        let before = factory.viewer.lock().get(agent).map(|kept| kept.opened.clone()).unwrap_or_default();
        opened_now(root, &before, crate::transcript::now_ms())
    });
    {
        let db = factory.store.db();
        match (&root, &opened) {
            (Some(root), Some(opened)) => db.execute(
                "INSERT INTO layouts (agent, layout, focus, opened) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (agent) DO UPDATE SET layout = excluded.layout, focus = excluded.focus, opened = excluded.opened",
                rusqlite::params![agent, root.encode_to_vec(), focus, serde_json::to_string(opened).unwrap_or_default()],
            )?,
            _ => db.execute("DELETE FROM layouts WHERE agent = ?1", [agent])?,
        };
    }
    {
        let mut layouts = factory.viewer.lock();
        match (root, opened) {
            (Some(root), Some(opened)) => layouts.insert(agent.to_owned(), Kept { root, focus: focus.to_owned(), opened }),
            _ => layouts.remove(agent),
        };
    }
    // Panes it no longer has leave where each device was reading.
    crate::presentation::layout_changed(factory, agent);
    // The files shown are watched (`annotations::monitor`).
    factory.wake_loops();
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// SetLayout: the person arranged the agent's panes, and maybe chose the
/// one in focus: a pane of the new layout, or "" for none. Left out, the
/// focus stays while its pane does.
pub(crate) fn set_layout(factory: &Factory, agent: &str, root: Option<v1::ViewNode>, focus: Option<&str>) -> Result<(), RequestError> {
    factory.known_agent(agent)?;
    if let Some(root) = &root {
        validate(root).map_err(RequestError::Layout)?;
    }
    let has = |id: &str| root.as_ref().is_some_and(|root| leaves(root).iter().any(|(leaf, _)| *leaf == id));
    let focus = match focus.map(str::trim) {
        Some(focus) if !focus.is_empty() && !has(focus) => {
            return Err(RequestError::Layout(format!("The layout has no pane \"{focus}\" to focus.")));
        }
        Some(focus) => focus.to_owned(),
        None => Some(factory.viewer.focus(agent)).filter(|kept| has(kept)).unwrap_or_default(),
    };
    store_layout(factory, agent, root, &focus)
}

/// PaneOpened: the person opened the agent's pane `pane`, which counts as
/// opened now.
pub(crate) fn pane_opened(factory: &Factory, agent: &str, pane: &str) -> Result<(), RequestError> {
    factory.known_agent(agent)?;
    let has = factory.viewer.layout(agent).is_some_and(|root| leaves(&root).iter().any(|(id, _)| *id == pane));
    if !has {
        return Err(RequestError::Layout(format!("The layout has no pane \"{pane}\".")));
    }
    reopened(factory, agent, pane)?;
    Ok(())
}

/// An agent is deleted, and its layout with it.
pub(crate) fn forget(factory: &Factory, agent: &str) -> rusqlite::Result<()> {
    factory.store.db().execute("DELETE FROM layouts WHERE agent = ?1", [agent])?;
    factory.viewer.lock().remove(agent);
    Ok(())
}

/// An agent showed the file of its pane `id` again: that opens it now, on
/// every device.
fn reopened(factory: &Factory, agent: &str, id: &str) -> rusqlite::Result<()> {
    let opened = {
        let mut layouts = factory.viewer.lock();
        let Some(kept) = layouts.get_mut(agent) else { return Ok(()) };
        kept.opened.insert(id.to_owned(), crate::transcript::now_ms());
        serde_json::to_string(&kept.opened).unwrap_or_default()
    };
    factory.store.db().execute("UPDATE layouts SET opened = ?2 WHERE agent = ?1", rusqlite::params![agent, opened])?;
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// `node`'s panes with when each was opened, as in `opened` (0 when not).
fn stamp(node: &mut v1::ViewNode, opened: &HashMap<String, i64>) {
    match &mut node.node {
        Some(Node::Leaf(leaf)) => leaf.opened_ms = opened.get(&node.id).copied().unwrap_or(0),
        Some(Node::Split(split)) => split.children.iter_mut().for_each(|child| stamp(child, opened)),
        None => {}
    }
}

/// When each pane of `root` was opened: as in `before`, else `now`.
fn opened_now(root: &v1::ViewNode, before: &HashMap<String, i64>, now: i64) -> HashMap<String, i64> {
    leaves(root).into_iter().map(|(id, _)| (id.to_owned(), before.get(id).copied().unwrap_or(now))).collect()
}

/// The pane show replaces in a full layout: the one opened longest ago
/// (in layout order between panes opened at once), never the conversation
/// or the pane in `focus`; None when there's no other.
fn oldest<'a>(root: &'a v1::ViewNode, opened: &HashMap<String, i64>, focus: &str) -> Option<(&'a str, &'a v1::ViewLeaf)> {
    leaves(root)
        .into_iter()
        .enumerate()
        .filter(|(_, (id, leaf))| leaf.kind != CHAT && *id != focus)
        .min_by_key(|(at, (id, _))| (opened.get(*id).copied().unwrap_or(0), *at))
        .map(|(_, pane)| pane)
}

/// `root` with the node `id` made `with`.
fn replace(node: &mut v1::ViewNode, id: &str, with: &v1::ViewNode) -> bool {
    if node.id == id {
        *node = with.clone();
        return true;
    }
    match &mut node.node {
        Some(Node::Split(split)) => split.children.iter_mut().any(|child| replace(child, id, with)),
        _ => false,
    }
}

/// What a pane shows, in a few words: its title, its file's name, or its kind.
fn named(leaf: &v1::ViewLeaf) -> String {
    if !leaf.title.trim().is_empty() {
        return leaf.title.trim().to_owned();
    }
    match Path::new(&leaf.path).file_name() {
        Some(name) if !leaf.path.is_empty() => name.to_string_lossy().into_owned(),
        // A page, as its pane's header names it in the app.
        _ => match leaf.kind.as_str() {
            "inbox" => "Needs you",
            "settings" => "Settings",
            "marketplace" => "Marketplace",
            "changelog" => "What's new",
            "map" => "Map",
            "usage" => "Usage",
            "monitors" => "Monitors",
            "files" => "Files",
            other => other,
        }
        .to_owned(),
    }
}

/// An id no node of `root` has: "pane-1", "pane-2"…
fn new_id(root: &v1::ViewNode, prefix: &str) -> String {
    fn ids<'a>(node: &'a v1::ViewNode, into: &mut HashSet<&'a str>) {
        into.insert(&node.id);
        if let Some(Node::Split(split)) = &node.node {
            for child in &split.children {
                ids(child, into);
            }
        }
    }
    let mut taken = HashSet::new();
    ids(root, &mut taken);
    (1..).map(|number| format!("{prefix}-{number}")).find(|id| !taken.contains(id.as_str())).unwrap_or_default()
}

fn split(id: String, axis: v1::ViewAxis, children: Vec<v1::ViewNode>) -> v1::ViewNode {
    v1::ViewNode { id, node: Some(Node::Split(v1::ViewSplit { axis: axis.into(), children, weights: Vec::new() })) }
}

/// `root` with `pane` added where viewers go: right of the conversation,
/// below the viewers already there.
fn add_pane(root: v1::ViewNode, pane: v1::ViewNode) -> v1::ViewNode {
    if is_plain(&root) {
        let id = new_id(&root, "split");
        return split(id, v1::ViewAxis::Row, vec![root, pane]);
    }
    let fresh = new_id(&root, "split");
    let mut root = root;
    let mut pane = Some(pane);
    beside_chat(&mut root, &mut pane, &fresh);
    root
}

/// Puts `pane` beside the conversation within `node`; returns whether the
/// conversation is in it.
fn beside_chat(node: &mut v1::ViewNode, pane: &mut Option<v1::ViewNode>, fresh: &str) -> bool {
    let Some(Node::Split(split_node)) = &mut node.node else { return false };
    let row = split_node.axis() == v1::ViewAxis::Row;
    let Some(at) = split_node.children.iter().position(|child| leaf_of(child).is_some_and(|leaf| leaf.kind == CHAT)) else {
        return split_node.children.iter_mut().any(|child| beside_chat(child, pane, fresh));
    };
    let Some(new) = pane.take() else { return true };
    match split_node.children.get_mut(at + 1) {
        // The viewers right of it: one more below them.
        Some(next) if row => match &mut next.node {
            Some(Node::Split(viewers)) if viewers.axis() == v1::ViewAxis::Column => {
                if !viewers.weights.is_empty() {
                    let mean = viewers.weights.iter().sum::<f64>() / viewers.weights.len() as f64;
                    viewers.weights.push(mean);
                }
                viewers.children.push(new);
            }
            _ => {
                let viewer = std::mem::take(next);
                *next = split(fresh.to_owned(), v1::ViewAxis::Column, vec![viewer, new]);
            }
        },
        // Nothing right of it: it's split in two.
        _ => {
            let chat = std::mem::take(&mut split_node.children[at]);
            split_node.children[at] = split(fresh.to_owned(), v1::ViewAxis::Row, vec![chat, new]);
        }
    }
    true
}

#[derive(Deserialize)]
struct ShowArgs {
    path: String,
    #[serde(default)]
    kind: Option<String>,
}

/// How long show waits for a file to open and its first bytes to be read.
const SHOW_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The show tool: opens the file in the agent's layout for the person. Its
/// details: the file's `path` and `kind`, and the `pane` (of `agent`'s
/// layout) it's in.
fn show(factory: &Factory, agent: &str, args: Value) -> Result<ToolResult, String> {
    let args: ShowArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let (folder, person, most) = {
        let inner = factory.lock();
        let me = super::caller(&inner.config, agent)?;
        let folder = crate::runner::places(&factory.home, &inner.config, me, &[]).folder;
        (folder, inner.config.person().to_owned(), crate::config::open_panes(inner.state.max_open_panes) as usize)
    };
    let asked = args.path.trim();
    if asked.is_empty() {
        return Err("path is empty: give the file to show.".to_owned());
    }
    let path = expand(asked);
    let path = if path.is_absolute() { path } else { folder.join(path) };
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    // Only a file: opening a named pipe, a socket or a device would hold
    // the agent's step (and Stop with it) until something wrote to it
    // . Finding out doesn't open it.
    if std::fs::metadata(&path).is_ok_and(|metadata| !metadata.is_file() && !metadata.is_dir()) {
        return Err(format!("{} isn't a file the app can show: it's a named pipe, a socket or a device.", path.display()));
    }
    let content = read(&path.display().to_string(), 0, 4096, false);
    if !content.error.is_empty() {
        let stop = if content.error.ends_with('.') { "" } else { "." };
        return Err(format!("{}{stop}", content.error));
    }
    let kind = match args.kind.as_deref().map(str::trim).filter(|kind| !kind.is_empty()) {
        Some(kind) if KINDS.contains(&kind) => kind,
        Some(kind) => return Err(format!("kind is \"text\", \"image\", \"video\" or \"pdf\", or left out; got \"{kind}\".")),
        None if is_image(&content.mime) => "image",
        None if is_video(&content.mime) => "video",
        None if is_pdf(&content.mime) => "pdf",
        None if is_text(&content.data) => "text",
        None => {
            return Err(format!("{} isn't text, an image, a video or a PDF the app can show ({}).", path.display(), content.mime));
        }
    };
    let path_text = path.display().to_string();
    let root = factory.viewer.layout(agent).unwrap_or_else(|| v1::ViewNode {
        id: CHAT.to_owned(),
        node: Some(Node::Leaf(v1::ViewLeaf { kind: CHAT.to_owned(), path: String::new(), title: String::new(), ..Default::default() })),
    });
    let open = leaves(&root).into_iter().find(|(_, leaf)| leaf.path == path_text && leaf.kind == kind).map(|(id, _)| id.to_owned());
    // What a full layout's new pane took the place of.
    let mut replaced: Option<v1::ViewLeaf> = None;
    let pane = match open {
        // Shown again, it counts as opened now (and is the last a full
        // layout gives up).
        Some(pane) => {
            reopened(factory, agent, &pane).map_err(|error| format!("Couldn't save the layout: {error}"))?;
            pane
        }
        None => {
            let id = new_id(&root, "pane");
            let title = path.file_name().map_or_else(|| path_text.clone(), |name| name.to_string_lossy().into_owned());
            let leaf = v1::ViewLeaf { kind: kind.to_owned(), path: path_text.clone(), title, ..Default::default() };
            let node = v1::ViewNode { id: id.clone(), node: Some(Node::Leaf(leaf)) };
            // A pane in focus stays so: the new one waits behind it.
            let focus = factory.viewer.focus(agent);
            // Full (every pane counts, whoever opened it: files, and pages
            // such as Settings), the new one takes the place, size and spot
            // of the pane opened longest ago, never the one in focus. A new
            // id, so no device carries its place in the old file over.
            let panes = leaves(&root).iter().filter(|(_, leaf)| leaf.kind != CHAT).count();
            let root = if panes >= most {
                let opened = factory.viewer.lock().get(agent).map(|kept| kept.opened.clone()).unwrap_or_default();
                let Some((old, leaf)) = oldest(&root, &opened, &focus).map(|(old, leaf)| (old.to_owned(), leaf.clone())) else {
                    return Err(format!(
                        "The only pane beside this conversation is the one {person} expanded to fill the window, and Settings → Display lets it have {most}: show it again once they've closed that pane or allowed more."
                    ));
                };
                let mut root = root;
                replace(&mut root, &old, &node);
                replaced = Some(leaf);
                root
            } else {
                add_pane(root, node)
            };
            validate(&root).map_err(|why| format!("The layout couldn't take it: {why}"))?;
            store_layout(factory, agent, Some(root), &focus).map_err(|error| format!("Couldn't save the layout: {error}"))?;
            id
        }
    };
    // A phone that opens it gets its preview without waiting for it.
    if kind == "image" || kind == "video" || kind == "pdf" {
        crate::previews::ahead(factory.home.root(), &path);
    }
    // The pane glows, so the person sees where it is.
    factory.send(Body::Highlight(v1::Highlight {
        targets: vec![v1::HighlightTarget { kind: "pane".to_owned(), id: format!("{agent}:{pane}") }],
        text: String::new(),
        from_agent_id: agent.to_owned(),
        kind: v1::HighlightKind::Agent.into(),
    }));
    let mut details = json!({"path": path_text, "kind": kind, "pane": pane, "agent": agent});
    let said = match &replaced {
        Some(old) => {
            details["replaced"] = json!({"kind": old.kind, "path": old.path});
            format!("Showing {path_text} to {person}, in place of {} (the oldest pane).", named(old))
        }
        None => format!("Showing {path_text} to {person}."),
    };
    Ok(ToolResult::text(said, details))
}

pub(crate) struct Viewer;

impl Extension for Viewer {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        // The manual's core and the tool's description say it all.
        Vec::new()
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let parameters = json!({
            "type": "object",
            "required": ["path"],
            "properties": {"path": {"type": "string"}, "kind": {"type": "string", "enum": KINDS}},
            "additionalProperties": false,
        });
        // Opened on a thread of its own, so a file that never answers (a
        // drive that's gone, say) holds up neither Claktory nor Stop, and
        // the agent hears about it after a while.
        let handler = agent_handler(factory, agent, |factory, agent, args| async move {
            let factory = live(factory)?;
            let asked = args.get("path").and_then(Value::as_str).unwrap_or_default().trim().to_owned();
            let opened = tokio::task::spawn_blocking(move || show(&factory, &agent, args));
            match tokio::time::timeout(SHOW_WAIT, opened).await {
                Ok(Ok(shown)) => shown.map_err(factory_agent::Error::Other),
                Ok(Err(error)) => Err(factory_agent::Error::Other(format!("Couldn't show {asked}: {error}"))),
                Err(_) => Err(factory_agent::Error::Other(format!(
                    "{asked} didn't open within {} seconds, so it isn't shown: is it on a drive or server that isn't answering?",
                    SHOW_WAIT.as_secs()
                ))),
            }
        });
        vec![tool("show", "Show a file", SHOW, parameters, handler)]
    }

    fn owns_tool(&self, name: &str) -> bool {
        name == "show"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(id: &str, kind: &str, path: &str) -> v1::ViewNode {
        v1::ViewNode {
            id: id.into(),
            node: Some(Node::Leaf(v1::ViewLeaf { kind: kind.into(), path: path.into(), title: String::new(), ..Default::default() })),
        }
    }

    /// The layout as nested text: "row(chat col(a b))".
    fn shape(node: &v1::ViewNode) -> String {
        match &node.node {
            Some(Node::Leaf(leaf)) if leaf.kind == CHAT => "chat".to_owned(),
            Some(Node::Leaf(leaf)) => leaf.path.clone(),
            Some(Node::Split(split)) => {
                let axis = if split.axis() == v1::ViewAxis::Row { "row" } else { "col" };
                format!("{axis}({})", split.children.iter().map(shape).collect::<Vec<_>>().join(" "))
            }
            None => "?".to_owned(),
        }
    }

    #[test]
    fn a_pane_is_named_as_its_header_reads() {
        let named = |kind: &str, path: &str, title: &str| {
            named(&v1::ViewLeaf { kind: kind.into(), path: path.into(), title: title.into(), ..Default::default() })
        };
        let pages = ["inbox", "settings", "marketplace", "changelog", "map", "usage", "monitors", "files"].map(|kind| named(kind, "", ""));
        assert_eq!(pages, ["Needs you", "Settings", "Marketplace", "What's new", "Map", "Usage", "Monitors", "Files"]);
        assert_eq!(named("text", "/Users/someone/notes/a.md", ""), "a.md");
        assert_eq!(named("app", "", "Budget"), "Budget");
    }

    #[test]
    fn viewers_open_right_of_the_conversation_and_stack_there() {
        let mut root = leaf("chat", CHAT, "");
        for name in ["a", "b", "c"] {
            let id = new_id(&root, "pane");
            root = add_pane(root, leaf(&id, "text", name));
            validate(&root).expect("valid");
        }
        assert_eq!(shape(&root), "row(chat col(a b c))");
        // Chat on the right: the viewer goes right of it all the same.
        let mut flipped = split("s".into(), v1::ViewAxis::Row, vec![leaf("x", "image", "x"), leaf("chat", CHAT, "")]);
        flipped = add_pane(flipped, leaf("pane-1", "text", "y"));
        assert_eq!(shape(&flipped), "row(x row(chat y))");
    }

    #[test]
    fn a_full_layout_gives_up_its_oldest_pane_but_never_the_one_in_focus() {
        let col = split("v".into(), v1::ViewAxis::Column, vec![leaf("a", "text", "a"), leaf("b", "text", "b"), leaf("c", "text", "c")]);
        let mut root = split("s".into(), v1::ViewAxis::Row, vec![leaf("chat", CHAT, ""), col]);
        let opened: HashMap<String, i64> = [("chat", 0), ("a", 30), ("b", 10), ("c", 20)].map(|(id, at)| (id.to_owned(), at)).into();
        assert_eq!(oldest(&root, &opened, "").map(|(id, _)| id), Some("b"));
        assert_eq!(oldest(&root, &opened, "b").map(|(id, _)| id), Some("c"), "the pane in focus is skipped");
        // Opened before times were kept: oldest, in layout order.
        assert_eq!(oldest(&root, &HashMap::new(), "").map(|(id, _)| id), Some("a"));
        assert!(replace(&mut root, "b", &leaf("pane-1", "text", "d")));
        assert_eq!(shape(&root), "row(chat col(a d c))", "in its place");
        let alone = split("s".into(), v1::ViewAxis::Row, vec![leaf("chat", CHAT, ""), leaf("a", "text", "a")]);
        assert!(oldest(&alone, &opened, "a").is_none());
        let opened_again = opened_now(&root, &opened, 99);
        assert_eq!((opened_again["a"], opened_again["pane-1"], opened_again.contains_key("b")), (30, 99, false));
    }

    #[test]
    fn layouts_need_one_conversation_unique_ids_and_sound_splits() {
        let good = split("s".into(), v1::ViewAxis::Row, vec![leaf("chat", CHAT, ""), leaf("p", "image", "/a.png")]);
        assert!(validate(&good).is_ok());
        let no_chat = split("s".into(), v1::ViewAxis::Row, vec![leaf("a", "text", "/a"), leaf("b", "text", "/b")]);
        assert!(validate(&no_chat).unwrap_err().contains("exactly once"));
        let twice = split("s".into(), v1::ViewAxis::Row, vec![leaf("a", CHAT, ""), leaf("b", CHAT, "")]);
        assert!(validate(&twice).unwrap_err().contains("2 times"));
        let same_id = split("s".into(), v1::ViewAxis::Row, vec![leaf("chat", CHAT, ""), leaf("chat", "text", "/b")]);
        assert!(validate(&same_id).unwrap_err().contains("Two panes"));
        let lone = split("s".into(), v1::ViewAxis::Row, vec![leaf("chat", CHAT, "")]);
        assert!(validate(&lone).unwrap_err().contains("two or more"));
        let mut weighted = good.clone();
        if let Some(Node::Split(split)) = &mut weighted.node {
            split.weights = vec![1.0];
        }
        assert!(validate(&weighted).unwrap_err().contains("weight"));
        let unknown_kind = split("s".into(), v1::ViewAxis::Row, vec![leaf("chat", CHAT, ""), leaf("d", "dashboard", "")]);
        assert!(validate(&unknown_kind).is_ok(), "kinds to come are kept");
    }

    #[test]
    fn files_are_sniffed_and_read_up_to_their_limit() {
        let dir = std::env::temp_dir().join(format!("factory-viewer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("notes.md"), "# Notes\nčaj\n").expect("write");
        std::fs::write(dir.join("pixel.png"), b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR").expect("write");
        std::fs::write(dir.join("blob.bin"), [0u8, 1, 2, 3]).expect("write");
        let text = read_file(&dir.join("notes.md").display().to_string(), 0, 0);
        assert_eq!(
            (text.mime.as_str(), text.data.as_slice(), text.truncated, text.error.as_str()),
            ("text/markdown", "# Notes\nčaj\n".as_bytes(), false, "")
        );
        assert!(text.modified_ms > 0 && text.size == 13);
        let cut = read_file(&dir.join("notes.md").display().to_string(), 0, 4);
        assert_eq!((cut.data.as_slice(), cut.truncated, cut.size), (&b"# No"[..], true, 13));
        assert_eq!(read_file(&dir.join("pixel.png").display().to_string(), 0, 0).mime, "image/png");
        assert_eq!(read_file(&dir.join("blob.bin").display().to_string(), 0, 0).mime, "application/octet-stream");
        assert!(read_file(&dir.join("gone.txt").display().to_string(), 0, 0).error.starts_with("No such file: "));
        assert!(read_file(&dir.display().to_string(), 0, 0).error.ends_with("is a folder"));
        assert!(read_file("relative.txt", 0, 0).error.contains("isn't a full path"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn movie(brand: &[u8; 4]) -> Vec<u8> {
        let mut bytes = b"\0\0\0\x18ftyp".to_vec();
        bytes.extend_from_slice(brand);
        bytes.extend_from_slice(b"\0\0\0\0isommp41\0\0\0\x08mdat");
        bytes
    }

    #[test]
    fn movies_are_sniffed_by_their_first_box_then_their_extension() {
        let at = |name: &str| PathBuf::from(format!("/clips/{name}"));
        assert_eq!(sniff(&movie(b"isom"), &at("a.mp4")), "video/mp4");
        assert_eq!(sniff(&movie(b"mp42"), &at("no-extension")), "video/mp4");
        assert_eq!(sniff(&movie(b"M4V "), &at("a.m4v")), "video/mp4");
        assert_eq!(sniff(&movie(b"qt  "), &at("a.mp4")), "video/quicktime", "the brand before the extension");
        assert_eq!(sniff(&movie(b"isom"), &at("a.mov")), "video/quicktime", "an unknown brand goes by the extension");
        assert_eq!(sniff(b"\0\0\0\x08wide\0\0\0\x10mdat", &at("old.MOV")), "video/quicktime");
        assert_eq!(sniff(b"\0\0\0\x08wide\0\0\0\x10mdat", &at("old.bin")), "application/octet-stream");
        assert_eq!(sniff(&movie(b"heic"), &at("photo.heic")), "application/octet-stream", "HEIF images aren't videos");
        assert_eq!(sniff(&movie(b"M4A "), &at("song.m4a")), "application/octet-stream");
        assert_eq!(sniff(b"\x1a\x45\xdf\xa3\x01\0\0\0", &at("a.webm")), "video/webm");
        assert!(is_video("video/mp4") && is_video("video/quicktime") && !is_video("video/webm"));
    }

    #[test]
    fn videos_are_read_a_range_at_a_time_with_the_whole_file_hash() {
        let dir = std::env::temp_dir().join(format!("factory-viewer-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("clip.mp4");
        let mut bytes = movie(b"isom");
        bytes.extend((0..RANGE_LIMIT + 1000).map(|at| (at % 251) as u8));
        std::fs::write(&path, &bytes).expect("write");
        let whole = crate::encoding::hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes));
        let name = path.display().to_string();

        let mut read = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() as u64 {
            let chunk = read_file(&name, offset, RANGE_LIMIT);
            assert_eq!(chunk.error, "");
            assert_eq!((chunk.mime.as_str(), chunk.offset, chunk.size), ("video/mp4", offset, bytes.len() as u64));
            assert!(chunk.truncated, "no range is the whole file");
            assert_eq!(chunk.sha256, whole, "every range has the whole file's hash");
            assert!(chunk.data.len() as u64 <= RANGE_LIMIT);
            offset += chunk.data.len() as u64;
            read.extend(chunk.data);
        }
        assert_eq!(read, bytes);
        let asked_more = read_file(&name, 0, 0);
        assert_eq!(asked_more.data.len() as u64, RANGE_LIMIT, "8 MB at most, whatever is asked");

        // A new version has a new hash, though it has the same size.
        bytes[100] ^= 0xff;
        std::fs::write(&path, &bytes).expect("rewrite");
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(&path).and_then(|file| file.set_modified(later)).expect("touch");
        let changed = read_file(&name, RANGE_LIMIT, 0);
        assert_eq!(changed.sha256, crate::encoding::hex(&<sha2::Sha256 as sha2::Digest>::digest(&bytes)));
        assert_eq!(whole_sha256(&path).expect("hash"), changed.sha256);

        // A small video in one read is whole.
        let small = dir.join("small.mov");
        std::fs::write(&small, movie(b"qt  ")).expect("write");
        let one = read_file(&small.display().to_string(), 0, 0);
        assert_eq!((one.mime.as_str(), one.truncated, one.sha256.len()), ("video/quicktime", false, 64));

        // Text from an offset: a range with the whole file's hash.
        let notes = dir.join("notes.txt");
        std::fs::write(&notes, "hello world").expect("write");
        let tail = read_file(&notes.display().to_string(), 6, 0);
        assert_eq!((tail.data.as_slice(), tail.offset, tail.truncated), (&b"world"[..], 6, true));
        assert_eq!(tail.sha256, crate::encoding::hex(&<sha2::Sha256 as sha2::Digest>::digest(b"hello world")));
        let past = read_file(&notes.display().to_string(), 50, 0);
        assert_eq!((past.data.len(), past.error.as_str()), (0, ""));

        // Over 2 GB (sparse, so nothing is written): refused.
        let huge = dir.join("huge.mp4");
        std::fs::write(&huge, movie(b"isom")).expect("write");
        std::fs::File::options().write(true).open(&huge).and_then(|file| file.set_len(VIDEO_LIMIT + 1)).expect("grow");
        assert!(read_file(&huge.display().to_string(), 0, 0).error.contains("too big"));
        assert!(whole_sha256(&huge).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
