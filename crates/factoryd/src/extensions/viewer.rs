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

use super::{Extension, Manifest, PromptSection, factory_tool};
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

const SHOW: &str = "Show the person a file in the app, in a pane beside your conversation: text (code, Markdown, logs, CSV…), an image, a video (MP4 or QuickTime) or a PDF. Use it instead of opening apps such as Preview or a browser on this machine: the person may be using Claktory from another device. path: absolute, or relative to your working folder. kind: \"text\", \"image\", \"video\" or \"pdf\", inferred when left out. A file already open isn't opened twice. The person sets how many panes can be open beside your conversation (Settings → Display): past that, show refuses until they close one. Returns at once.";

pub(crate) const SCHEMA: &str = "
-- How each agent's main area is laid out (the Viewer extension): its
-- conversation and the viewers beside it, as an encoded ViewNode.
CREATE TABLE layouts (agent TEXT PRIMARY KEY, layout BLOB NOT NULL);
";

/// The migration that added the pane shown in focus (#272): a leaf's id
/// in the layout, '' for none.
pub(crate) const FOCUS: &str = "
ALTER TABLE layouts ADD COLUMN focus TEXT NOT NULL DEFAULT '';
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

/// An agent's layout and the pane shown in focus ('' for none).
#[derive(Clone)]
struct Kept {
    root: v1::ViewNode,
    focus: String,
}

impl State {
    /// The layouts in factory.db; those that can't be read are left out.
    pub(crate) fn load(store: &Store) -> State {
        let mut layouts = HashMap::new();
        let read = || -> rusqlite::Result<Vec<(String, Vec<u8>, String)>> {
            let db = store.db();
            let mut statement = db.prepare("SELECT agent, layout, focus FROM layouts")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            rows.collect()
        };
        match read() {
            Ok(rows) => {
                for (agent, bytes, focus) in rows {
                    match v1::ViewNode::decode(bytes.as_slice()) {
                        Ok(root) => {
                            // A focus that names no pane is none.
                            let focus = if leaves(&root).iter().any(|(id, _)| *id == focus) { focus } else { String::new() };
                            layouts.insert(agent, Kept { root, focus });
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

    /// The agent's layout; None for just its conversation.
    pub(crate) fn layout(&self, agent: &str) -> Option<v1::ViewNode> {
        self.lock().get(agent).map(|kept| kept.root.clone())
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
fn expand(path: &str) -> PathBuf {
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
    let mut content = v1::FileContent { path: path.display().to_string(), ..v1::FileContent::default() };
    let failed = |mut content: v1::FileContent, error: String| {
        content.error = error;
        content
    };
    if !path.is_absolute() {
        return failed(content, format!("{} isn't a full path: start it with / or ~", path.display()));
    }
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return failed(content, format!("No such file: {}", path.display())),
        Err(error) => return failed(content, format!("Can't read {}: {}", path.display(), plain(&error))),
    };
    if metadata.is_dir() {
        return failed(content, format!("{} is a folder", path.display()));
    }
    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) => return failed(content, format!("Can't read {}: {}", path.display(), plain(&error))),
    };
    let mut start = Vec::new();
    if let Err(error) = (&mut file).take(512).read_to_end(&mut start) {
        return failed(content, format!("Can't read {}: {}", path.display(), plain(&error)));
    }
    let mime = sniff(&start, &path);
    let ranged = offset > 0 || is_video(mime) || is_pdf(mime);
    if is_pdf(mime) && metadata.len() > PDF_LIMIT {
        let megabytes = metadata.len().div_ceil(1_000_000);
        return failed(content, format!("This PDF is too big to open here ({megabytes} MB)."));
    }
    if ranged && metadata.len() > VIDEO_LIMIT {
        return failed(content, format!("{} is too big to play. Claktory plays videos up to 2 GB.", path.display()));
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
        return failed(content, format!("Can't read {}: {}", path.display(), plain(&error)));
    }
    let rest = limit.saturating_sub(data.len() as u64);
    if let Err(error) = file.take(rest).read_to_end(&mut data) {
        return failed(content, format!("Can't read {}: {}", path.display(), plain(&error)));
    }
    content.mime = mime.to_owned();
    content.size = metadata.len();
    content.offset = offset;
    content.truncated = offset > 0 || (data.len() as u64) < metadata.len();
    if !content.truncated {
        content.sha256 = crate::encoding::hex(&<sha2::Sha256 as sha2::Digest>::digest(&data));
    } else if ranged && hash {
        match whole_sha256(&path) {
            Ok(sha) => content.sha256 = sha,
            Err(error) => return failed(content, format!("Can't read {}: {}", path.display(), plain(&error))),
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
    let root = root.filter(|root| !is_plain(root));
    let focus = if root.is_some() { focus } else { "" };
    {
        let db = factory.store.db();
        match &root {
            Some(root) => db.execute(
                "INSERT INTO layouts (agent, layout, focus) VALUES (?1, ?2, ?3)
                 ON CONFLICT (agent) DO UPDATE SET layout = excluded.layout, focus = excluded.focus",
                rusqlite::params![agent, root.encode_to_vec(), focus],
            )?,
            None => db.execute("DELETE FROM layouts WHERE agent = ?1", [agent])?,
        };
    }
    {
        let mut layouts = factory.viewer.lock();
        match root {
            Some(root) => layouts.insert(agent.to_owned(), Kept { root, focus: focus.to_owned() }),
            None => layouts.remove(agent),
        };
    }
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

/// An agent is deleted, and its layout with it.
pub(crate) fn forget(factory: &Factory, agent: &str) -> rusqlite::Result<()> {
    factory.store.db().execute("DELETE FROM layouts WHERE agent = ?1", [agent])?;
    factory.viewer.lock().remove(agent);
    Ok(())
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
    let pane = match open {
        Some(pane) => pane,
        None => {
            // Full, it refuses rather than take a pane the person may be
            // reading; a file already open still shows (above). Every pane
            // counts, whoever opened it: files, and pages such as Settings.
            let panes = leaves(&root).iter().filter(|(_, leaf)| leaf.kind != CHAT).count();
            if panes >= most {
                return Err(format!(
                    "This conversation already has {panes} panes open beside it, the most show opens at once ({most}; {person} can change it in Settings → Display). Ask {person} to close one, or show an already-open file to point at it."
                ));
            }
            let id = new_id(&root, "pane");
            let title = path.file_name().map_or_else(|| path_text.clone(), |name| name.to_string_lossy().into_owned());
            let leaf = v1::ViewLeaf { kind: kind.to_owned(), path: path_text.clone(), title, ..Default::default() };
            let root = add_pane(root, v1::ViewNode { id: id.clone(), node: Some(Node::Leaf(leaf)) });
            validate(&root).map_err(|why| format!("The layout couldn't take it: {why}"))?;
            // A pane in focus stays so: the new one waits behind it.
            let focus = factory.viewer.focus(agent);
            store_layout(factory, agent, Some(root), &focus).map_err(|error| format!("Couldn't save the layout: {error}"))?;
            id
        }
    };
    // The pane glows, so the person sees where it is.
    factory.send(Body::Highlight(v1::Highlight {
        targets: vec![v1::HighlightTarget { kind: "pane".to_owned(), id: format!("{agent}:{pane}") }],
        text: String::new(),
        from_agent_id: agent.to_owned(),
        kind: v1::HighlightKind::Agent.into(),
    }));
    let details = json!({"path": path_text, "kind": kind, "pane": pane, "agent": agent});
    Ok(ToolResult::text(format!("Showing {path_text} to {person}."), details))
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
        vec![factory_tool(factory, agent, "show", "Show a file", SHOW, parameters, show)]
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
