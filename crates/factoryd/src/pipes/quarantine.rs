//! Quarantine (threat model §7, P1): a pipe member that can only read and
//! answer the pipe. Its tools are pipe_send, pipe_read and pipe_status, the
//! file tools inside the pipe's folder (reading its inbox, writing only in
//! its outbox, never through a symbolic link) and ask_person; it has no
//! bash, no secrets, no web or messaging, and no other extension's tools,
//! extension_call included. Its system prompt names no project folder.
//!
//! Which agents are quarantined is kept with the pipe (`Record.quarantined`,
//! in factory.db), never in config.json, so only the person changes it, with
//! UpdatePipe from the app: no agent tool reaches it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use factory_agent::Tool;
use factory_agent::tools::ToolKind;
use serde_json::Value;

use super::{Record, change, folder_of, line};
use crate::config::Config;
use crate::factory::{Factory, host_event};
use crate::runner::Places;

/// What a quarantined agent may reach.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Quarantine {
    /// Where it works: its first pipe's outbox, writable.
    pub folder: PathBuf,
    /// Its other pipes' outboxes, writable too.
    pub writable: Vec<PathBuf>,
    /// Its pipes' inboxes, to read only.
    pub readable: Vec<PathBuf>,
    /// Its pipes, by id.
    pub pipes: Vec<String>,
}

impl Quarantine {
    /// Where its tools work, in place of its project's folders.
    pub(crate) fn places(&self) -> Places {
        Places { folder: self.folder.clone(), extra_dirs: self.writable.clone(), read_only: self.readable.clone() }
    }
}

/// The extensions a quarantined agent keeps (when they're on for it).
pub(crate) const KEPT_EXTENSIONS: &[&str] = &[crate::extensions::ask::ID];

/// The pipe tools a quarantined agent keeps.
const KEPT_PIPE_TOOLS: &[&str] = &["pipe_send", "pipe_read", "pipe_status"];

/// `agent`'s quarantine, if any pipe it's in has it quarantined (any pipe
/// kept, closed ones too: it has read them). `config` is the one the caller
/// holds, as pipes are read under Factory's lock.
pub(crate) fn quarantine(factory: &Factory, config: &Config, agent: &str) -> Option<Quarantine> {
    let records: Vec<Record> = factory.pipes.all().into_iter().filter(|record| record.quarantined.iter().any(|id| id == agent)).collect();
    if records.is_empty() {
        return None;
    }
    let mut outboxes = Vec::new();
    let mut inboxes = Vec::new();
    for record in &records {
        if let Some(folder) = folder_of(config, record) {
            outboxes.push(folder.join("outbox"));
            inboxes.push(folder.join("inbox"));
        }
    }
    let folder = if outboxes.is_empty() {
        // Its pipes' project is gone: an empty folder of its own.
        let nowhere = factory.home.root().join("pipes").join("quarantined").join(agent);
        let _ = std::fs::create_dir_all(&nowhere);
        nowhere
    } else {
        outboxes.remove(0)
    };
    Some(Quarantine { folder, writable: outboxes, readable: inboxes, pipes: records.into_iter().map(|record| record.id).collect() })
}

/// The tools of a quarantined agent, but for its extensions' (Ask's, from
/// the caller) and extension_call: the file tools in its places (its
/// quarantine's), with writes refused through a symbolic link, and the pipe
/// tools.
pub(crate) fn tools(factory: &Arc<Factory>, agent: &str, places: &Places) -> Vec<Tool> {
    let mut tools = factory_agent::coding_tools(&places.folder, &places.extra_dirs, None);
    factory_agent::tools::read_only(&mut tools, &places.read_only);
    tools.retain(|tool| tool.kind != ToolKind::Bash);
    let roots: Vec<PathBuf> = std::iter::once(places.folder.clone()).chain(places.extra_dirs.iter().cloned()).collect();
    let mut tools: Vec<Tool> = tools
        .into_iter()
        .map(
            |tool| {
                if matches!(tool.kind, ToolKind::WriteFile | ToolKind::EditFile) { no_links(tool, &places.folder, &roots) } else { tool }
            },
        )
        .collect();
    tools.extend(pipe_tools(factory, agent));
    tools
}

/// The pipe tools a quarantined agent keeps.
pub(crate) fn pipe_tools(factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
    super::agents::tools(factory, agent).into_iter().filter(|tool| KEPT_PIPE_TOOLS.contains(&tool.spec.name.as_ref())).collect()
}

/// `tool` (write_file or edit_file), refusing a path that is, or passes
/// through, a symbolic link, or whose temporary file would be one: the file
/// tools check where a link points, but a write could still follow one.
fn no_links(tool: Tool, folder: &Path, roots: &[PathBuf]) -> Tool {
    let spec = tool.spec.clone();
    let folder = folder.to_path_buf();
    let roots = roots.to_vec();
    let handler: factory_agent::tools::Handler = Arc::new(move |args: Value| {
        let (tool, folder, roots) = (tool.clone(), folder.clone(), roots.clone());
        Box::pin(async move {
            let input = args.get("path").and_then(Value::as_str).unwrap_or_default().to_owned();
            if let Some(link) = link_in(&folder, &roots, &input) {
                return Err(factory_agent::Error::Other(format!(
                    "Not written: {} is a symbolic link, and a quarantined agent's file tools never write through one.",
                    link.display()
                )));
            }
            tool.execute(&args, &tokio_util::sync::CancellationToken::new(), &|_| {}).await
        })
    });
    let mut wrapped = Tool::custom(spec.name.clone(), spec.label.clone(), spec.description.clone(), spec.parameters.clone(), handler);
    wrapped.spec = spec;
    wrapped
}

/// The first symbolic link on the way to `input` from the root it's in (the
/// root included), or its temporary file; None when there's none, or when
/// it's in no root (the tool itself refuses that).
fn link_in(folder: &Path, roots: &[PathBuf], input: &str) -> Option<PathBuf> {
    let target = factory_agent::tools::fs::resolve(folder, input);
    let root = roots.iter().filter(|root| target.starts_with(root)).max_by_key(|root| root.as_os_str().len())?;
    let is_link = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink());
    let mut path = root.clone();
    if is_link(&path) {
        return Some(path);
    }
    for part in target.strip_prefix(root).ok()?.components() {
        path.push(part);
        if is_link(&path) {
            return Some(path);
        }
    }
    let temporary = PathBuf::from(format!("{}.tmp", target.display()));
    is_link(&temporary).then_some(temporary)
}

/// The line its system prompt's pipe section gets.
pub(crate) fn prompt_line(record: &Record) -> String {
    format!(
        "You're limited to the pipe \"{}\": you can only read and answer it, and draft files in its outbox. Your tools are the pipe's, the file tools in its folder (its inbox to read, its outbox to write) and ask_person: no commands, no other files, no secrets, web or messaging.",
        super::agents::escape(&record.name)
    )
}

/// Tells `agent` it's quarantined for `record` now, or released from it.
fn tell(factory: &Arc<Factory>, agent: &str, record: &Record, quarantined: bool) {
    let person = factory.lock().config.person_capitalized().to_owned();
    let name = &record.name;
    let (summary, text) = if quarantined {
        (
            format!("Claktory now limits you to the pipe \"{name}\""),
            format!(
                "Claktory now limits you to the pipe \"{name}\": you can only read and answer it (pipe_read, pipe_send, pipe_status), read the files in its inbox, write files in its outbox, and ask {person} (ask_person, when it's on for you). You can't run commands, reach other files, use secrets, the web or other agents. Only {person} can lift this, in the pipe's settings."
            ),
        )
    } else {
        (
            format!("{person} no longer limits you to the pipe \"{name}\""),
            format!(
                "{person} no longer limits you to the pipe \"{name}\": your usual tools and folders are back from your next step. What you read from the pipe stays in your conversation, so using a secret still needs {person}'s approval each time until it's cleared."
            ),
        )
    };
    factory.deliver_event(agent, text, host_event(summary));
}

/// Sets the pipe's quarantined members to `ids` (members only) and tells
/// each agent that came in or left. The person's alone: UpdatePipe,
/// ApprovePipe, a liaison they add.
pub(crate) fn set(factory: &Arc<Factory>, pipe: &str, ids: Vec<String>) {
    let Some((before, after)) = change(factory, pipe, |record| {
        let before = std::mem::take(&mut record.quarantined);
        for id in ids {
            if record.members.contains(&id) && !record.quarantined.contains(&id) {
                record.quarantined.push(id);
            }
        }
        (before, record.quarantined.clone())
    }) else {
        return;
    };
    let Some(record) = factory.pipes.get(pipe) else { return };
    let name = |id: &str| factory.lock().config.agent(id).map_or_else(|| id.to_owned(), |agent| agent.name.clone());
    for id in after.iter().filter(|id| !before.contains(id)) {
        tell(factory, id, &record, true);
    }
    for id in before.iter().filter(|id| !after.contains(id)) {
        // Removed from the pipe: it isn't told of a pipe it left.
        if record.members.contains(id) {
            tell(factory, id, &record, false);
            line(
                factory,
                pipe,
                format!(
                    "{} is no longer limited to this pipe: it can run commands, reach your files and use secrets on this computer.",
                    name(id)
                ),
            );
        }
    }
}

/// At start, once: a pipe kept from before quarantine quarantines each
/// member whose role is "Liaison", whatever its prompt, and tells it.
pub(crate) fn upgrade(factory: &Arc<Factory>) {
    let config = factory.lock().config.clone();
    for record in factory.pipes.all() {
        if record.quarantine_set {
            continue;
        }
        let liaisons: Vec<String> = record
            .members
            .iter()
            .filter(|id| config.agent(id).is_some_and(|agent| agent.role.trim().eq_ignore_ascii_case("Liaison")))
            .cloned()
            .collect();
        let mut ids = record.quarantined.clone();
        ids.extend(liaisons.into_iter().filter(|id| !record.quarantined.contains(id)));
        change(factory, &record.id, |record| record.quarantine_set = true);
        if ids != record.quarantined {
            set(factory, &record.id, ids);
        }
    }
}
