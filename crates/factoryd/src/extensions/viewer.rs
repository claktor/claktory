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
    description: "Lets it show you files, such as text and images, beside its conversation.",
    default_enabled: true,
    options: &[],
};

const SHOW: &str = "Show the person a file in the app, in a pane beside your conversation: text (code, Markdown, logs, CSV…) or an image. Use it instead of opening apps such as Preview or a browser on this machine: the person may be using Claktory from another device. path: absolute, or relative to your working folder. kind: \"text\" or \"image\", inferred when left out. A file already open isn't opened twice. Returns at once.";

pub(crate) const SCHEMA: &str = "
-- How each agent's main area is laid out (the Viewer extension): its
-- conversation and the viewers beside it, as an encoded ViewNode.
CREATE TABLE layouts (agent TEXT PRIMARY KEY, layout BLOB NOT NULL);
";

/// Text is read up to this much, whatever is asked…
const TEXT_LIMIT: u64 = 2_000_000;
/// …and images up to this much.
const IMAGE_LIMIT: u64 = 20_000_000;
/// Panes in one layout, the conversation included.
const MAX_LEAVES: usize = 12;
/// Splits within splits.
const MAX_DEPTH: usize = 8;

const CHAT: &str = "chat";
const KINDS: [&str; 2] = ["text", "image"];

/// Agents' layouts, as kept in factory.db.
#[derive(Default)]
pub(crate) struct State {
    layouts: Mutex<HashMap<String, v1::ViewNode>>,
}

impl State {
    /// The layouts in factory.db; those that can't be read are left out.
    pub(crate) fn load(store: &Store) -> State {
        let mut layouts = HashMap::new();
        let read = || -> rusqlite::Result<Vec<(String, Vec<u8>)>> {
            let db = store.db();
            let mut statement = db.prepare("SELECT agent, layout FROM layouts")?;
            let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        };
        match read() {
            Ok(rows) => {
                for (agent, bytes) in rows {
                    match v1::ViewNode::decode(bytes.as_slice()) {
                        Ok(root) => {
                            layouts.insert(agent, root);
                        }
                        Err(error) => tracing::warn!(%agent, %error, "an agent's layout can't be read; it shows just its conversation"),
                    }
                }
            }
            Err(error) => tracing::error!(%error, "could not read the agents' layouts"),
        }
        State { layouts: Mutex::new(layouts) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, v1::ViewNode>> {
        self.layouts.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The agent's layout; None for just its conversation.
    pub(crate) fn layout(&self, agent: &str) -> Option<v1::ViewNode> {
        self.lock().get(agent).cloned()
    }

    /// The paths of every agent's panes of this kind, each once.
    pub(crate) fn paths(&self, kind: &str) -> HashSet<String> {
        let layouts = self.lock();
        let all = layouts.values().flat_map(leaves);
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

/// Reads up to `max_bytes` of a file (0: the default), for a viewer.
pub(crate) fn read_file(path: &str, max_bytes: u64) -> v1::FileContent {
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
    let limit = if is_image(mime) { IMAGE_LIMIT } else { TEXT_LIMIT };
    let limit = if max_bytes == 0 { limit } else { max_bytes.min(limit) };
    let mut data = start;
    data.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let rest = limit.saturating_sub(data.len() as u64);
    if let Err(error) = file.take(rest).read_to_end(&mut data) {
        return failed(content, format!("Can't read {}: {}", path.display(), plain(&error)));
    }
    content.mime = mime.to_owned();
    content.size = metadata.len();
    content.truncated = (data.len() as u64) < metadata.len();
    if !content.truncated {
        content.sha256 = crate::encoding::hex(&<sha2::Sha256 as sha2::Digest>::digest(&data));
    }
    content.data = data;
    content.modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX));
    content
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

/// Saves the agent's layout (None: just its conversation) and shows it on
/// every device.
fn store_layout(factory: &Factory, agent: &str, root: Option<v1::ViewNode>) -> Result<(), RequestError> {
    let root = root.filter(|root| !is_plain(root));
    {
        let db = factory.store.db();
        match &root {
            Some(root) => db.execute(
                "INSERT INTO layouts (agent, layout) VALUES (?1, ?2) ON CONFLICT (agent) DO UPDATE SET layout = excluded.layout",
                rusqlite::params![agent, root.encode_to_vec()],
            )?,
            None => db.execute("DELETE FROM layouts WHERE agent = ?1", [agent])?,
        };
    }
    {
        let mut layouts = factory.viewer.lock();
        match root {
            Some(root) => layouts.insert(agent.to_owned(), root),
            None => layouts.remove(agent),
        };
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(())
}

/// SetLayout: the person arranged the agent's panes.
pub(crate) fn set_layout(factory: &Factory, agent: &str, root: Option<v1::ViewNode>) -> Result<(), RequestError> {
    if factory.lock().config.agent(agent).is_none() {
        return Err(RequestError::UnknownAgent(agent.to_owned()));
    }
    if let Some(root) = &root {
        validate(root).map_err(RequestError::Layout)?;
    }
    store_layout(factory, agent, root)
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
/// below the viewers already there. At the most panes, it takes the place
/// of the first viewer.
fn add_pane(root: v1::ViewNode, pane: v1::ViewNode) -> v1::ViewNode {
    if leaves(&root).len() >= MAX_LEAVES {
        return replace_first_viewer(root, pane).0;
    }
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

/// `node` with its first viewer pane replaced by `pane`; whether it was.
fn replace_first_viewer(mut node: v1::ViewNode, pane: v1::ViewNode) -> (v1::ViewNode, Option<v1::ViewNode>) {
    match &mut node.node {
        Some(Node::Leaf(leaf)) if leaf.kind != CHAT => (pane, None),
        Some(Node::Split(split)) => {
            let mut pane = Some(pane);
            for child in &mut split.children {
                if let Some(new) = pane.take() {
                    let (replaced, left) = replace_first_viewer(std::mem::take(child), new);
                    *child = replaced;
                    pane = left;
                }
            }
            (node, pane)
        }
        _ => (node, Some(pane)),
    }
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
    let folder = {
        let inner = factory.lock();
        let Some(me) = inner.config.agent(agent) else { return Err("You're no longer in this Claktory.".to_owned()) };
        crate::runner::places(&factory.home, &inner.config, me).folder
    };
    let asked = args.path.trim();
    if asked.is_empty() {
        return Err("path is empty: give the file to show.".to_owned());
    }
    let path = expand(asked);
    let path = if path.is_absolute() { path } else { folder.join(path) };
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let content = read_file(&path.display().to_string(), 4096);
    if !content.error.is_empty() {
        return Err(format!("{}.", content.error));
    }
    let kind = match args.kind.as_deref().map(str::trim).filter(|kind| !kind.is_empty()) {
        Some(kind) if KINDS.contains(&kind) => kind,
        Some(kind) => return Err(format!("kind is \"text\" or \"image\", or left out; got \"{kind}\".")),
        None if is_image(&content.mime) => "image",
        None if is_text(&content.data) => "text",
        None => {
            return Err(format!(
                "{} is neither text nor an image the app shows ({}), so it can't be shown yet.",
                path.display(),
                content.mime
            ));
        }
    };
    let path_text = path.display().to_string();
    let root = factory.viewer.layout(agent).unwrap_or_else(|| v1::ViewNode {
        id: CHAT.to_owned(),
        node: Some(Node::Leaf(v1::ViewLeaf { kind: CHAT.to_owned(), path: String::new(), title: String::new() })),
    });
    let open = leaves(&root).into_iter().find(|(_, leaf)| leaf.path == path_text && leaf.kind == kind).map(|(id, _)| id.to_owned());
    let pane = match open {
        Some(pane) => pane,
        None => {
            let id = new_id(&root, "pane");
            let title = path.file_name().map_or_else(|| path_text.clone(), |name| name.to_string_lossy().into_owned());
            let leaf = v1::ViewLeaf { kind: kind.to_owned(), path: path_text.clone(), title };
            let root = add_pane(root, v1::ViewNode { id: id.clone(), node: Some(Node::Leaf(leaf)) });
            validate(&root).map_err(|why| format!("The layout couldn't take it: {why}"))?;
            store_layout(factory, agent, Some(root)).map_err(|error| format!("Couldn't save the layout: {error}"))?;
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
    let person = factory.lock().config.person().to_owned();
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
        v1::ViewNode { id: id.into(), node: Some(Node::Leaf(v1::ViewLeaf { kind: kind.into(), path: path.into(), title: String::new() })) }
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
        // At the most panes, the first viewer makes way.
        let mut full = leaf("chat", CHAT, "");
        for number in 0..MAX_LEAVES + 2 {
            let id = new_id(&full, "pane");
            full = add_pane(full, leaf(&id, "text", &format!("f{number}")));
        }
        assert_eq!(leaves(&full).len(), MAX_LEAVES);
        validate(&full).expect("valid");
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
        let text = read_file(&dir.join("notes.md").display().to_string(), 0);
        assert_eq!(
            (text.mime.as_str(), text.data.as_slice(), text.truncated, text.error.as_str()),
            ("text/markdown", "# Notes\nčaj\n".as_bytes(), false, "")
        );
        assert!(text.modified_ms > 0 && text.size == 13);
        let cut = read_file(&dir.join("notes.md").display().to_string(), 4);
        assert_eq!((cut.data.as_slice(), cut.truncated, cut.size), (&b"# No"[..], true, 13));
        assert_eq!(read_file(&dir.join("pixel.png").display().to_string(), 0).mime, "image/png");
        assert_eq!(read_file(&dir.join("blob.bin").display().to_string(), 0).mime, "application/octet-stream");
        assert!(read_file(&dir.join("gone.txt").display().to_string(), 0).error.starts_with("No such file: "));
        assert!(read_file(&dir.display().to_string(), 0).error.ends_with("is a folder"));
        assert!(read_file("relative.txt", 0).error.contains("isn't a full path"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
