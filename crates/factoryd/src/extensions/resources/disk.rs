//! What Factory's data and its projects take on disk. Walking folders is
//! slow, so it runs in the background now and then, never when asked.

use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use factory_protocol::v1;

/// What to measure, read from the config when a measurement starts.
pub(crate) struct Plan {
    pub home: PathBuf,
    /// Id and name.
    pub agents: Vec<(String, String)>,
    pub projects: Vec<Project>,
}

pub(crate) struct Project {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Name and path, for folders outside the project's own.
    pub folders: Vec<(String, PathBuf)>,
}

/// Sizes by path, so a folder counted twice (a project inside the
/// workspace) is walked once; every walk stops at `deadline`.
struct Walker {
    deadline: Instant,
    sizes: HashMap<PathBuf, (u64, bool)>,
}

impl Walker {
    /// The bytes `path` takes, and whether the walk was cut short. Symlinks
    /// aren't followed; what can't be read counts as nothing.
    fn size(&mut self, path: &Path) -> (u64, bool) {
        if let Some(size) = self.sizes.get(path) {
            return *size;
        }
        let (mut bytes, mut partial) = (0, false);
        let mut stack = vec![path.to_owned()];
        while let Some(at) = stack.pop() {
            if Instant::now() > self.deadline {
                partial = true;
                break;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&at) else { continue };
            // What it takes, not how long it says it is (sparse files, clones).
            bytes += metadata.blocks() * 512;
            if metadata.is_dir()
                && let Ok(entries) = std::fs::read_dir(&at)
            {
                stack.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
            }
        }
        self.sizes.insert(path.to_owned(), (bytes, partial));
        (bytes, partial)
    }

    /// The sizes of `paths` together.
    fn sum<'a>(&mut self, paths: impl IntoIterator<Item = &'a Path>) -> (u64, bool) {
        paths.into_iter().fold((0, false), |(bytes, partial), path| {
            let (more, cut) = self.size(path);
            (bytes + more, partial || cut)
        })
    }
}

fn usage(label: impl Into<String>, kind: &str, id: &str, path: &Path, (bytes, partial): (u64, bool)) -> v1::DiskUsage {
    v1::DiskUsage {
        label: label.into(),
        kind: kind.to_owned(),
        id: id.to_owned(),
        path: path.display().to_string(),
        bytes,
        parts: Vec::new(),
        partial,
    }
}

/// What's directly in `dir`, as paths.
fn children(dir: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> =
        std::fs::read_dir(dir).map(|entries| entries.filter_map(Result::ok).map(|entry| entry.path()).collect()).unwrap_or_default();
    paths.sort();
    paths
}

fn largest_first(rows: &mut [v1::DiskUsage]) {
    rows.sort_by_key(|row| std::cmp::Reverse(row.bytes));
}

/// Factory's data (its home folder, by part), its releases, each agent's
/// data, then each project's folders, largest first within each.
pub(crate) fn measure(plan: &Plan, deadline: Instant) -> Vec<v1::DiskUsage> {
    let mut walker = Walker { deadline, sizes: HashMap::new() };
    let home = &plan.home;
    let (agents_dir, trees, releases, workspace) = (home.join("agents"), home.join("trees"), home.join("releases"), home.join("workspace"));

    // Each agent: its folder (conversations, logs, database) and its checkouts.
    let mut agents = Vec::new();
    for (id, name) in &plan.agents {
        let folder = agents_dir.join(id);
        let files = children(&folder);
        let part = |walker: &mut Walker, label: &str, kind: &str, paths: Vec<&Path>| {
            let size = walker.sum(paths);
            usage(label, kind, id, &folder, size)
        };
        let conversations: Vec<&Path> = files
            .iter()
            .filter(|path| {
                let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
                name.starts_with("segment-") || name.starts_with("transcript-") || name == "archive"
            })
            .map(PathBuf::as_path)
            .collect();
        let database: Vec<&Path> = files
            .iter()
            .filter(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("database.sqlite")))
            .map(PathBuf::as_path)
            .collect();
        let (tasks, monitors, checkout) = (folder.join("tasks"), folder.join("monitors"), trees.join(id));
        let mut parts = vec![
            part(&mut walker, "Conversations", "conversations", conversations),
            part(&mut walker, "Task logs", "logs", vec![&tasks]),
            part(&mut walker, "Monitor logs", "logs", vec![&monitors]),
            part(&mut walker, "Database", "database", database),
        ];
        parts.push(usage("Checkout", "checkout", id, &checkout, walker.size(&checkout)));
        parts.retain(|part| part.bytes > 0);
        largest_first(&mut parts);
        let total = walker.sum([folder.as_path(), checkout.as_path()]);
        let mut agent = usage(name.clone(), "agent", id, &folder, total);
        agent.parts = parts;
        agents.push(agent);
    }
    largest_first(&mut agents);

    // Factory's home, by part: what's left over is everything else in it.
    let records: Vec<PathBuf> = children(home)
        .into_iter()
        .filter(|path| path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("factory.db")))
        .collect();
    let mut parts = vec![
        usage("Releases", "releases", "", &releases, walker.size(&releases)),
        usage("Agents' conversations, logs and databases", "agents", "", &agents_dir, walker.size(&agents_dir)),
        usage("Checkouts", "checkout", "", &trees, walker.size(&trees)),
        usage("Claktory's records", "records", "", home, walker.sum(records.iter().map(PathBuf::as_path))),
        usage("Workspace", "workspace", "", &workspace, walker.sum(children(&workspace).iter().map(PathBuf::as_path))),
    ];
    let (all, partial) = walker.sum(children(home).iter().map(PathBuf::as_path));
    let counted: u64 = parts.iter().map(|part| part.bytes).sum();
    parts.push(usage("Everything else", "other", "", home, (all.saturating_sub(counted), false)));
    parts.retain(|part| part.bytes > 0);
    largest_first(&mut parts);
    let mut rows = vec![v1::DiskUsage { parts, ..usage("Claktory's data", "home", "", home, (all, partial)) }];
    rows.push(usage("Releases", "releases", "", &releases, walker.size(&releases)));
    rows.extend(agents);

    let mut projects = Vec::new();
    for project in &plan.projects {
        projects.push(usage(project.name.clone(), "project", &project.id, &project.path, walker.size(&project.path)));
        for (name, path) in &project.folders {
            projects.push(usage(format!("{} › {name}", project.name), "folder", &project.id, path, walker.size(path)));
        }
    }
    largest_first(&mut projects);
    rows.extend(projects);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factorys_data_is_measured_by_part_and_agent() {
        let home = std::env::temp_dir().join(format!("factory-disk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        for (path, bytes) in [
            ("agents/scout/segment-0001.json", 40_000),
            ("agents/scout/tasks/task-1.log", 200_000),
            ("agents/scout/database.sqlite", 100_000),
            ("trees/scout/repo/main.rs", 10_000),
            ("releases/2026.11.20-1012/factoryd", 300_000),
            ("factory.db", 50_000),
            ("workspace/site/index.html", 20_000),
        ] {
            let path = home.join(path);
            std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
            std::fs::write(path, vec![7; bytes]).expect("write");
        }
        let plan = Plan {
            home: home.clone(),
            agents: vec![("scout".into(), "Scout".into())],
            projects: vec![Project { id: "site".into(), name: "Site".into(), path: home.join("workspace/site"), folders: Vec::new() }],
        };
        let rows = measure(&plan, Instant::now() + std::time::Duration::from_secs(10));
        let kinds: Vec<(&str, &str)> = rows.iter().map(|row| (row.kind.as_str(), row.label.as_str())).collect();
        assert_eq!(kinds, [("home", "Claktory's data"), ("releases", "Releases"), ("agent", "Scout"), ("project", "Site")]);
        let parts: Vec<&str> = rows[0].parts.iter().map(|part| part.label.as_str()).collect();
        assert_eq!(parts[0], "Agents' conversations, logs and databases");
        assert!(parts.contains(&"Releases") && parts.contains(&"Claktory's records") && parts.contains(&"Workspace"), "{parts:?}");
        let scout: Vec<&str> = rows[2].parts.iter().map(|part| part.label.as_str()).collect();
        assert_eq!(scout, ["Task logs", "Database", "Conversations", "Checkout"]);
        assert!(rows[2].bytes >= 350_000 && rows[0].bytes >= rows[2].bytes + 300_000, "{rows:?}");
        assert!(rows.iter().all(|row| !row.partial));
        // Out of time: what it got so far, marked.
        let cut = measure(&plan, Instant::now());
        assert!(cut[0].partial);
        let _ = std::fs::remove_dir_all(&home);
    }
}
