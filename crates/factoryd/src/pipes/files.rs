//! Reading a pipe's files taints the reader (pipes-automation.md §6, H7).
//! What another Claktory sent, and what a liaison or a curator wrote from
//! it, is as untrusted on disk as in pipe_read, so read_file, read_image or
//! edit_file (whose failures and diffs show its lines)
//! on any of these taints the agent the same way:
//!
//! - a pipe's folder in its project (its inbox and outbox), by that pipe;
//! - `<home>/pipes/incoming/<pipe>/`, where a message's files arrive, by
//!   that pipe;
//! - `<home>/pipes/reports/<pipe>/`, a hand-off's attachments, by that pipe;
//! - a curator's own folder (its database too), by everything the curator
//!   has read, each through it.
//!
//! Bash can still read them untainted: this catches honest mistakes, not a
//! determined agent. The defence stays structural: privileged agents aren't
//! pointed at pipe files, and hand-offs never go through files.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::factory::Factory;

/// What reading under a folder taints by.
enum Source {
    Pipe(String),
    Curator(String),
}

/// `agent` is about to read `args.path` with read_file or read_image
/// (`agents::gate`): tainted first if it's a pipe's file.
pub(crate) fn reading(factory: &Arc<Factory>, agent: &str, args: &Value) {
    let Some(input) = args.get("path").and_then(Value::as_str) else { return };
    let (folder, roots) = {
        let inner = factory.lock();
        let Some(found) = inner.config.agent(agent) else { return };
        (crate::runner::placed(factory, &inner.config, found).folder, roots(factory, &inner.config))
    };
    let target = factory_agent::tools::fs::resolve(&folder, input);
    let targets: Vec<PathBuf> = std::iter::once(target.clone()).chain(std::fs::canonicalize(&target).ok()).collect();
    for (root, source) in roots {
        let mut forms = vec![root.clone()];
        forms.extend(std::fs::canonicalize(&root).ok());
        if !targets.iter().any(|target| forms.iter().any(|root| target.starts_with(root))) {
            continue;
        }
        match source {
            Source::Pipe(pipe) => super::agents::taint(factory, agent, &pipe),
            Source::Curator(curator) => super::agents::spread_taint(factory, &curator, agent),
        }
    }
}

/// Every folder whose files taint, with what they taint by.
fn roots(factory: &Factory, config: &crate::config::Config) -> Vec<(PathBuf, Source)> {
    let pipes = factory.home.root().join("pipes");
    let mut roots = Vec::new();
    let records = factory.pipes.all();
    for record in &records {
        if let Some(folder) = super::folder_of(config, record) {
            roots.push((folder, Source::Pipe(record.id.clone())));
        }
        for kept in ["incoming", "reports"] {
            roots.push((pipes.join(kept).join(crate::pipe_link::sanitize(&record.id)), Source::Pipe(record.id.clone())));
            if crate::pipe_link::sanitize(&record.id) != record.id {
                roots.push((pipes.join(kept).join(&record.id), Source::Pipe(record.id.clone())));
            }
        }
    }
    // What's there for a pipe that's gone still came from it.
    for kept in ["incoming", "reports"] {
        for entry in std::fs::read_dir(pipes.join(kept)).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !records.iter().any(|record| record.id == name || crate::pipe_link::sanitize(&record.id) == name) {
                roots.push((entry.path(), Source::Pipe(name)));
            }
        }
    }
    for curator in factory.pipes.curators() {
        roots.push((factory.home.agent_dir(&curator), Source::Curator(curator)));
    }
    roots
}

