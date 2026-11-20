//! The focused project, and the file explorers of the projects whose files
//! are open.
//!
//! The explorer shows the project's folder as the agents see it, minus what
//! the folder's `.factoryignore` hides (gitignore syntax). Without one, common
//! build and dependency folders are hidden. Which folders are open, and
//! whether hidden files show, is workspace state, so every device shows the
//! same tree; Factory lists the open folders again every tick and sends the
//! tree whenever it changes.
//!
//! The project's folders (where some of its agents work) are listed too,
//! each as a root of its own, and so are its agents' own checkouts. No
//! directory shows twice: two roots with the same path show once, a root
//! inside another isn't listed in the other's tree, and the project's own
//! root is empty when one of its folders is the project's folder itself.

use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use factory_protocol::v1::{self, server_message::Body};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::config::{Config, FolderView};
use crate::factory::{Factory, Inner, RequestError};

pub const IGNORE_FILE: &str = ".factoryignore";

/// Written into new projects, and used for folders without a `.factoryignore`.
pub const DEFAULT_IGNORE: &str = "\
# Hidden in Claktory's Files unless \"Show hidden files\" is on.
# Same syntax as .gitignore.
.git/
.DS_Store
node_modules/
target/
build/
dist/
.dart_tool/
.next/
.venv/
venv/
__pycache__/
*.pyc
.gradle/
.idea/
coverage/
";

impl Factory {
    pub async fn focus_project(&self, id: &str) -> Result<(), RequestError> {
        {
            let mut inner = self.lock();
            if id.is_empty() {
                inner.state.focused_project = None;
            } else {
                if inner.config.project(id).is_none() {
                    return Err(RequestError::UnknownProject(id.to_owned()));
                }
                inner.state.focused_project = Some(id.to_owned());
                let selected = inner.state.selected_agent.as_deref().and_then(|agent| inner.config.agent(agent));
                if selected.is_none_or(|agent| agent.project.as_deref() != Some(id)) {
                    let first = inner.config.agents.iter().find(|agent| agent.project.as_deref() == Some(id));
                    let first = first.map(|agent| agent.id.clone());
                    inner.state.selected_agent = first;
                }
            }
        }
        let selected = self.lock().state.selected_agent.clone();
        if let Some(agent) = selected {
            self.ensure_loaded(&agent).await?;
        }
        {
            let mut inner = self.lock();
            // The newly selected agent's panel.
            let view = self.current_view(&inner);
            self.set_view(&mut inner, view);
            self.broadcast_workspace(&inner);
            if let Some(transcript) = inner.state.selected_agent.as_deref().and_then(|agent| crate::factory::transcript_of(&inner, agent)) {
                self.send(Body::Transcript(transcript));
            }
        }
        self.save_state()?;
        self.refresh_explorer().await;
        Ok(())
    }

    /// Opens or closes a folder: in the project's own folder, with `folder`
    /// in that folder of the project, or with `agent` in that agent's own
    /// checkout.
    pub async fn expand_folder(&self, project: &str, folder: &str, agent: &str, path: &str, expanded: bool) -> Result<(), RequestError> {
        let relative = Path::new(path);
        if path.is_empty() || !relative.components().all(|component| matches!(component, Component::Normal(_))) {
            return Err(RequestError::NotInProject(path.to_owned()));
        }
        if !agent.is_empty() {
            let inner = self.lock();
            if inner.config.agent(agent).is_none_or(|config| config.project.as_deref() != Some(project)) {
                return Err(RequestError::UnknownAgent(agent.to_owned()));
            }
        } else if !folder.is_empty() {
            let inner = self.lock();
            if inner.config.project(project).is_some_and(|config| config.folder(folder).is_none()) {
                return Err(RequestError::UnknownFolder { project: project.to_owned(), folder: folder.to_owned() });
            }
        }
        let key = if agent.is_empty() { view_key(project, folder) } else { checkout_key(project, agent) };
        self.change_view(&key, |view| {
            if expanded {
                view.expanded.insert(path.to_owned());
            } else {
                // Closing a folder closes what's inside it too, like VS Code.
                let inside = format!("{path}/");
                view.expanded.retain(|open| open != path && !open.starts_with(&inside));
            }
        })
        .await
    }

    pub async fn show_all_files(&self, project: &str, show_all: bool) -> Result<(), RequestError> {
        self.change_view(project, |view| view.show_all = show_all).await
    }

    /// Changes one root's view: `key` is a project id, or `view_key` of one of its folders.
    async fn change_view(&self, key: &str, change: impl FnOnce(&mut FolderView)) -> Result<(), RequestError> {
        {
            let mut inner = self.lock();
            let project = key.split_once('/').map_or(key, |(project, _)| project);
            if inner.config.project(project).is_none() {
                return Err(RequestError::UnknownProject(project.to_owned()));
            }
            let view = inner.state.explorer.entry(key.to_owned()).or_default();
            change(view);
            if *view == FolderView::default() {
                inner.state.explorer.remove(key);
            }
        }
        self.save_state()?;
        self.refresh_explorer().await;
        Ok(())
    }

    fn save_state(&self) -> Result<(), RequestError> {
        self.store.save_state(&self.lock().state)?;
        Ok(())
    }

    /// Lists the folders of every project whose files are open again, and
    /// sends each tree that changed. The focused project's goes last, and
    /// again after any other's, for apps that keep only the last one.
    pub(crate) async fn refresh_explorer(&self) {
        let _listing = self.explorer_lock.lock().await;
        let (plans, focused) = {
            let inner = self.lock();
            let mut open: BTreeSet<String> = self.viewer.paths(FILES_PANE).into_iter().collect();
            open.extend(self.file_watches.projects());
            open.extend(inner.state.focused_project.clone());
            let plans: Vec<Plan> = open.iter().filter_map(|id| self.plan(&inner, id)).collect();
            (plans, inner.state.focused_project.clone())
        };
        let listed = tokio::task::spawn_blocking(move || plans.into_iter().map(Plan::list).collect::<Vec<v1::Explorer>>()).await;
        let listed = match listed {
            Ok(listed) => listed,
            Err(error) => {
                tracing::error!(%error, "listing the explorer failed");
                return;
            }
        };
        let mut inner = self.lock();
        inner.explorers.retain(|id, _| listed.iter().any(|explorer| explorer.project_id == *id));
        let mut send_focused = false;
        for explorer in listed {
            if inner.explorers.get(&explorer.project_id) == Some(&explorer) {
                continue;
            }
            if focused.as_deref() == Some(explorer.project_id.as_str()) {
                send_focused = true;
            } else {
                self.send(Body::Explorer(explorer.clone()));
                send_focused |= focused.is_some();
            }
            inner.explorers.insert(explorer.project_id.clone(), explorer);
        }
        if send_focused && let Some(explorer) = focused.and_then(|id| inner.explorers.get(&id)) {
            self.send(Body::Explorer(explorer.clone()));
        }
    }

    /// What to list for a project's explorer; None if there's no such project.
    fn plan(&self, inner: &Inner, id: &str) -> Option<Plan> {
        let project = inner.config.project(id)?;
        let view = inner.state.explorer.get(&project.id).cloned().unwrap_or_default();
        let mut roots: Vec<(v1::ExplorerRoot, PathBuf, FolderView)> = Vec::new();
        for folder in &project.folders {
            if roots.iter().any(|(_, path, _)| *path == folder.path) {
                continue;
            }
            let mut own = inner.state.explorer.get(&view_key(&project.id, &folder.id)).cloned().unwrap_or_default();
            own.show_all = view.show_all;
            let root = v1::ExplorerRoot { folder_id: folder.id.clone(), ..v1::ExplorerRoot::default() };
            roots.push((root, folder.path.clone(), own));
        }
        let agents = inner.config.agents.iter().filter(|agent| agent.project.as_deref() == Some(project.id.as_str()));
        for agent in agents {
            let Some(checkout) = crate::runner::checkout(&self.home, &inner.config, agent) else { continue };
            if roots.iter().any(|(_, path, _)| *path == checkout.folder) {
                continue;
            }
            let mut own = inner.state.explorer.get(&checkout_key(&project.id, &agent.id)).cloned().unwrap_or_default();
            own.show_all = view.show_all;
            let root = v1::ExplorerRoot { agent_id: agent.id.clone(), branch: checkout.branch, ..v1::ExplorerRoot::default() };
            roots.push((root, checkout.folder, own));
        }
        Some(Plan { id: project.id.clone(), root: project.path.clone(), view, roots })
    }

    /// The projects' explorers to send a client on connect: the focused
    /// project's last.
    pub(crate) fn explorers(&self, inner: &Inner) -> Vec<v1::Explorer> {
        let focused = inner.state.focused_project.as_deref();
        let (mut explorers, last): (Vec<_>, Vec<_>) =
            inner.explorers.values().cloned().partition(|explorer| Some(explorer.project_id.as_str()) != focused);
        explorers.extend(last);
        explorers
    }

    /// WatchFiles: the connection shows `project`'s files (none if empty).
    pub(crate) async fn watch_files(&self, watch: &Watch, project: &str) -> Result<(), RequestError> {
        if !project.is_empty() && self.lock().config.project(project).is_none() {
            return Err(RequestError::UnknownProject(project.to_owned()));
        }
        self.file_watches.set(watch.id, project);
        self.refresh_explorer().await;
        Ok(())
    }
}

/// The kind of pane (ViewLeaf) that shows a project's files; its path is the
/// project's id.
const FILES_PANE: &str = "files";

/// A project's explorer, to be listed.
struct Plan {
    id: String,
    root: PathBuf,
    view: FolderView,
    roots: Vec<(v1::ExplorerRoot, PathBuf, FolderView)>,
}

impl Plan {
    fn list(self) -> v1::Explorer {
        let Plan { id, root, view, roots } = self;
        // Each root's tree leaves out the other roots inside it.
        let paths: Vec<PathBuf> = roots.iter().map(|(_, path, _)| path.clone()).collect();
        let (entries, error) = if paths.contains(&root) { (Vec::new(), String::new()) } else { listed(&root, &view, &paths) };
        let mut folders = Vec::new();
        for (root, path, own) in roots {
            let (entries, error) = listed(&path, &own, &paths);
            folders.push(v1::ExplorerRoot { path: path.display().to_string(), entries, error, ..root });
        }
        v1::Explorer { project_id: id, show_all: view.show_all, entries, error, folders }
    }
}

/// The projects connections watch (WatchFiles), one at most for each.
#[derive(Default)]
pub(crate) struct Watches {
    next: AtomicU64,
    by_connection: Mutex<HashMap<u64, String>>,
}

impl Watches {
    fn lock(&self) -> MutexGuard<'_, HashMap<u64, String>> {
        self.by_connection.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A connection's watch, which watches nothing until set and is
    /// forgotten when dropped.
    pub(crate) fn watch(factory: &Arc<Factory>) -> Watch {
        Watch { id: factory.file_watches.next.fetch_add(1, Ordering::Relaxed), factory: Arc::downgrade(factory) }
    }

    fn set(&self, id: u64, project: &str) {
        let mut watches = self.lock();
        if project.is_empty() {
            watches.remove(&id);
        } else {
            watches.insert(id, project.to_owned());
        }
    }

    fn projects(&self) -> Vec<String> {
        self.lock().values().cloned().collect()
    }
}

/// What one connection watches.
pub(crate) struct Watch {
    id: u64,
    factory: Weak<Factory>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some(factory) = self.factory.upgrade() {
            factory.file_watches.set(self.id, "");
        }
    }
}

/// Where the state keeps the view of one of a project's folders
/// ("site/research"), or of the project's own (`folder` empty). Ids have no "/".
pub(crate) fn view_key(project: &str, folder: &str) -> String {
    if folder.is_empty() { project.to_owned() } else { format!("{project}/{folder}") }
}

/// Where the state keeps the view of an agent's own checkout ("site/:builder").
fn checkout_key(project: &str, agent: &str) -> String {
    format!("{project}/:{agent}")
}

/// Whether the root a view key names is still there: a project, one of its
/// folders, or one of its agents (whose checkout it is).
pub(crate) fn view_exists(config: &Config, key: &str) -> bool {
    let Some((project, root)) = key.split_once('/') else { return true };
    match root.strip_prefix(':') {
        Some(agent) => config.agent(agent).is_some_and(|agent| agent.project.as_deref() == Some(project)),
        None => config.project(project).is_some_and(|project| project.folder(root).is_some()),
    }
}

/// A root's rows, or why it can't be read. Directories in `roots` (the
/// explorer's other roots) are left out.
fn listed(root: &Path, view: &FolderView, roots: &[PathBuf]) -> (Vec<v1::FileEntry>, String) {
    match list(root, view, roots) {
        Ok(entries) => (entries, String::new()),
        Err(error) => (Vec::new(), format!("Can't read {}: {error}", root.display())),
    }
}

/// The rows to show: the folder's entries, and the entries of each open
/// folder right after it.
fn list(root: &Path, view: &FolderView, roots: &[PathBuf]) -> std::io::Result<Vec<v1::FileEntry>> {
    let rules = rules(root);
    let mut rows = Vec::new();
    let listing = Listing { root, rules: &rules, view, roots };
    listing.folder("", 0, false, &mut rows)?;
    Ok(rows)
}

/// One root being listed.
struct Listing<'a> {
    root: &'a Path,
    rules: &'a Gitignore,
    view: &'a FolderView,
    /// Directories shown as roots of their own, left out here.
    roots: &'a [PathBuf],
}

impl Listing<'_> {
    fn folder(&self, folder: &str, depth: u32, in_hidden: bool, rows: &mut Vec<v1::FileEntry>) -> std::io::Result<()> {
        let (root, rules, view) = (self.root, self.rules, self.view);
        let mut children = Vec::new();
        for entry in std::fs::read_dir(root.join(folder))? {
            let Ok(entry) = entry else { continue };
            let name = entry.file_name().to_string_lossy().into_owned();
            // Follows symlinks, so a linked folder opens like a folder.
            let is_dir = std::fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_dir());
            if is_dir && self.roots.contains(&entry.path()) {
                continue;
            }
            let hidden = in_hidden || rules.matched(entry.path(), is_dir).is_ignore();
            if hidden && !view.show_all {
                continue;
            }
            let path = if folder.is_empty() { name.clone() } else { format!("{folder}/{name}") };
            children.push(v1::FileEntry { path, name, depth, is_dir, expanded: false, hidden });
        }
        children.sort_by(|a, b| {
            b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name))
        });
        for mut child in children {
            child.expanded = child.is_dir && view.expanded.contains(&child.path);
            let open = child.expanded.then(|| (child.path.clone(), child.hidden));
            rows.push(child);
            if let Some((path, hidden)) = open {
                // An unreadable folder opens empty rather than hiding the whole tree.
                if let Err(error) = self.folder(&path, depth + 1, hidden, rows) {
                    tracing::debug!(%error, %path, "can't list folder");
                }
            }
        }
        Ok(())
    }
}

fn rules(root: &Path) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    let file = root.join(IGNORE_FILE);
    if file.is_file() {
        if let Some(error) = builder.add(&file) {
            tracing::warn!(%error, file = %file.display(), "part of .factoryignore was skipped");
        }
    } else {
        for line in DEFAULT_IGNORE.lines() {
            if let Err(error) = builder.add_line(None, line) {
                tracing::warn!(%error, line, "bad default ignore rule");
            }
        }
    }
    match builder.build() {
        Ok(rules) => rules,
        Err(error) => {
            tracing::warn!(%error, file = %file.display(), "can't use .factoryignore");
            Gitignore::empty()
        }
    }
}
