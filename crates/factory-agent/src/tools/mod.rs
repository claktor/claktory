pub mod bash;
pub mod checks;
pub mod diff;
pub mod fs;
pub mod images;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::errors::{AgentError, AgentFailure, Error};
use crate::schema;
use crate::secrets::Secrets;
use crate::stream::BoxFuture;
use crate::text::{utf16_head, utf16_len};
use crate::types::ToolResult;

pub const MAX_OUTPUT: usize = 24_000;

pub fn bounded(text: &str) -> String {
    if utf16_len(text) <= MAX_OUTPUT {
        text.to_owned()
    } else {
        format!("{}\n[Output truncated. Request a smaller range.]", utf16_head(text, MAX_OUTPUT))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: Cow<'static, str>,
    pub label: Cow<'static, str>,
    pub description: Cow<'static, str>,
    pub parameters: Value,
    pub constrained_sampling: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolKind {
    ReadFile,
    ReadImage,
    WriteFile,
    EditFile,
    Bash,
    /// Provided by the program running the agent.
    Custom,
}

pub type Update<'a> = &'a (dyn Fn(ToolResult) + Sync);

/// Runs a custom tool on its validated arguments.
pub type Handler = Arc<dyn Fn(Value) -> BoxFuture<Result<ToolResult, Error>> + Send + Sync>;

#[derive(Clone)]
pub struct Tool {
    pub spec: ToolSpec,
    pub kind: ToolKind,
    cwd: PathBuf,
    /// Absolute directories outside `cwd` that the file tools may also address by absolute path.
    extra_dirs: Vec<PathBuf>,
    /// Absolute directories read_file may also address, and write_file and edit_file may not
    /// change, even inside `cwd` (see `read_only`).
    read_only: Vec<PathBuf>,
    /// Absolute directories no file tool may address at all, inside `cwd`
    /// or not, by any path or link (see `denied`).
    denied: Vec<PathBuf>,
    handler: Option<Handler>,
    background: Option<bash::Background>,
}

impl std::fmt::Debug for Tool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tool").field("spec", &self.spec).field("kind", &self.kind).field("cwd", &self.cwd).finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct FilePath {
    path: String,
}

impl Tool {
    fn new(cwd: &Path, kind: ToolKind, name: &'static str, label: &'static str, description: &'static str) -> Tool {
        let spec = ToolSpec {
            name: name.into(),
            label: label.into(),
            description: description.into(),
            parameters: schema::parameters(name),
            constrained_sampling: json!({"type": "json_schema", "strict": "prefer"}),
        };
        Tool {
            spec,
            kind,
            cwd: cwd.to_path_buf(),
            extra_dirs: Vec::new(),
            read_only: Vec::new(),
            denied: Vec::new(),
            handler: None,
            background: None,
        }
    }

    /// A tool the program running the agent provides. `parameters` is its
    /// JSON schema; nullable properties are the optional ones.
    pub fn custom(
        name: impl Into<Cow<'static, str>>,
        label: impl Into<Cow<'static, str>>,
        description: impl Into<Cow<'static, str>>,
        parameters: Value,
        handler: Handler,
    ) -> Tool {
        let spec = ToolSpec {
            name: name.into(),
            label: label.into(),
            description: description.into(),
            parameters,
            constrained_sampling: json!({"type": "json_schema", "strict": "prefer"}),
        };
        Tool {
            spec,
            kind: ToolKind::Custom,
            cwd: PathBuf::new(),
            extra_dirs: Vec::new(),
            read_only: Vec::new(),
            denied: Vec::new(),
            handler: Some(handler),
            background: None,
        }
    }

    pub async fn execute(&self, args: &Value, cancel: &CancellationToken, update: Update<'_>) -> Result<ToolResult, Error> {
        self.execute_detachable(args, cancel, &CancellationToken::new(), update).await
    }

    /// Runs the tool; `detach` moves a bash command still running to the
    /// background, as its timeout does. Other tools ignore it.
    pub async fn execute_detachable(
        &self,
        args: &Value,
        cancel: &CancellationToken,
        detach: &CancellationToken,
        update: Update<'_>,
    ) -> Result<ToolResult, Error> {
        self.execute_with(args, cancel, detach, update, None).await
    }

    /// Runs the tool, as `execute_detachable`; with `secrets`, bash can set
    /// the secrets a call names in its command's environment, and hides
    /// their values (all values `secrets` knows) in its output.
    pub async fn execute_with(
        &self,
        args: &Value,
        cancel: &CancellationToken,
        detach: &CancellationToken,
        update: Update<'_>,
        secrets: Option<&Secrets>,
    ) -> Result<ToolResult, Error> {
        let mut cwd = &self.cwd;
        let file = matches!(self.kind, ToolKind::ReadFile | ToolKind::ReadImage | ToolKind::WriteFile | ToolKind::EditFile);
        if file && !self.denied.is_empty() {
            let address: FilePath = serde_json::from_value(args.clone())?;
            if is_denied(&fs::resolve(&self.cwd, &address.path), &self.denied).await {
                return Err(AgentError::new(AgentFailure::Private { path: address.path }).into());
            }
        }
        if file && !(self.extra_dirs.is_empty() && self.read_only.is_empty()) {
            let address: FilePath = serde_json::from_value(args.clone())?;
            let target = fs::resolve(&self.cwd, &address.path);
            // The most specific directory the path is in decides whether it may change.
            let mut within: Option<(&PathBuf, bool)> = None;
            let writable = std::iter::once(&self.cwd).chain(&self.extra_dirs).map(|dir| (dir, true));
            for (dir, can_write) in writable.chain(self.read_only.iter().map(|dir| (dir, false))) {
                if target.starts_with(dir) && within.is_none_or(|(best, _)| dir.as_os_str().len() > best.as_os_str().len()) {
                    within = Some((dir, can_write));
                }
            }
            if let Some((dir, can_write)) = within {
                if !can_write && !matches!(self.kind, ToolKind::ReadFile | ToolKind::ReadImage) {
                    return Err(AgentError::new(AgentFailure::ReadOnly { path: address.path }).into());
                }
                if Path::new(&address.path).is_absolute() {
                    cwd = dir;
                }
            }
        }
        match self.kind {
            ToolKind::ReadFile => fs::read_file(cwd, args, cancel).await,
            ToolKind::ReadImage => images::read_image(cwd, args, cancel).await,
            ToolKind::WriteFile => fs::write_file(cwd, args, cancel).await,
            ToolKind::EditFile => fs::edit_file(cwd, args, cancel).await,
            ToolKind::Bash => {
                let folders: Vec<&Path> =
                    std::iter::once(&self.cwd).chain(&self.extra_dirs).chain(&self.read_only).map(PathBuf::as_path).collect();
                bash::run(&self.cwd, &folders, args, cancel, detach, update, self.background.as_ref(), secrets).await
            }
            ToolKind::Custom => match &self.handler {
                Some(handler) => handler(args.clone()).await,
                None => Err(Error::Other(format!("{} has no handler", self.spec.name))),
            },
        }
    }
}

/// The coding agent's system prompt. `extra_dirs` are the directories given to `coding_tools`.
pub fn coding_prompt(cwd: &Path, extra_dirs: &[PathBuf]) -> String {
    let mut prompt = format!(
        "You are a coding agent working in {}. Your shell starts there; don't cd there first. Use relative paths. Messages wrapped in <event> weren't written by the person: they're from the program running you, another agent, or a background task that finished. Never make a git worktree, clone or copy of the repository you work in; when you need a checkout of your own, ask for Claktory's Own checkout extension.",
        cwd.display()
    );
    if !extra_dirs.is_empty() {
        let mut dirs = Vec::new();
        for dir in extra_dirs {
            dirs.push(dir.display().to_string());
        }
        prompt.push_str(&format!(" read_file, write_file and edit_file also accept absolute paths in {}.", dirs.join(", ")));
    }
    prompt
}

/// The coding tools: read, write and edit files in `cwd`, and run bash there. `extra_dirs` are absolute
/// directories outside `cwd` that the file tools also accept absolute paths into, with the same symlink checks.
/// With `background`, bash can also run commands in the background.
pub fn coding_tools(cwd: &Path, extra_dirs: &[PathBuf], background: Option<bash::Background>) -> Vec<Tool> {
    let mut tools = vec![
        Tool::new(
            cwd,
            ToolKind::ReadFile,
            "read_file",
            "Read file",
            "Read numbered UTF-8 lines. offset is one-based; limit defaults to 200. Use smaller ranges when output is truncated.",
        ),
        Tool::new(
            cwd,
            ToolKind::ReadImage,
            "read_image",
            "Read image",
            "Look at an image file: PNG, JPEG, GIF or WebP. It's added to the conversation as an image you see, at this point. One too big for your model is scaled down for you; the result says so. Only an agent whose model reads images can use it.",
        ),
        Tool::new(
            cwd,
            ToolKind::WriteFile,
            "write_file",
            "Write file",
            "Create or replace a whole UTF-8 file and its parent directories. Use edit_file for partial changes to an existing file.",
        ),
        Tool::new(
            cwd,
            ToolKind::EditFile,
            "edit_file",
            "Edit file",
            "Apply exact replacements to an existing file and return a unified diff. Each oldText must match exactly once in the original file, or set replaceAll to replace every occurrence; a unique match that differs only in leading or trailing whitespace on each line is accepted. Edits must not overlap; errors name each failing edit.",
        ),
    ];
    tools.push(Tool::new(cwd, ToolKind::Bash, "bash", "Run command", "Run bash. Every command starts in your working folder, so don't cd there first; a cd lasts only for that command. Unhandled failures stop the script (errexit and pipefail); use if or || for expected failures. Piping to head can cause SIGPIPE; use a draining filter such as sed -n instead. command is the shell script itself, not a bash -c/bash -lc invocation — put the script text directly in command. Run tests and inspect git diff. timeout is seconds (default 120, maximum 600). A command still running at its timeout moves to the background and keeps running, and you get an event when it ends (one that starts with sleep is stopped instead). Set run_in_background up front for servers, watchers and anything you know is long: it returns at once with a task id and a log file, and the command is stopped after 30 minutes unless timeout says otherwise (up to 7200). Never wait with sleep: background tasks and messages wake you by themselves. Output up to 30,000 characters comes back whole; past that you get its start, its end and the path of the whole output. Every result ends with the exit status and how long the command took; tool updates contain output as it arrives. Files written by commands inherit the environment's resource limits."));
    for tool in &mut tools {
        tool.extra_dirs = extra_dirs.to_vec();
        if tool.kind == ToolKind::Bash {
            tool.background.clone_from(&background);
        }
    }
    tools
}

/// Gives bash its `secrets` parameter, for an agent that can be given
/// secrets: it names secrets granted to the agent, whose variables are set
/// in that command's environment only (see `Tool::execute_with`). Without
/// it, bash's schema is as it always was.
pub fn secrets(tools: &mut [Tool]) {
    for tool in tools.iter_mut().filter(|tool| tool.kind == ToolKind::Bash) {
        if let Some(properties) = tool.spec.parameters.get_mut("properties").and_then(Value::as_object_mut) {
            properties.insert("secrets".into(), json!({"anyOf": [{"type": "array", "items": {"type": "string"}}, {"type": "null"}]}));
        }
        if !tool.spec.description.contains(SECRETS_HINT) {
            tool.spec.description = format!("{} {SECRETS_HINT}", tool.spec.description).into();
        }
    }
}

/// Tells bash's description about the programs Claktory gives every agent
/// (ripgrep, jq), each only when it resolves on `path`, the PATH the
/// agent's commands run with, as its context starts: one can be missing (a
/// build not installed from a release, a Linux install without its tools),
/// and the description never claims a program the machine lacks, or agents
/// learn it fails and stop trying it.
pub fn installed(tools: &mut [Tool], path: &str) {
    let hints: Vec<&str> = INSTALLED.iter().filter(|(program, _)| on_path(program, path)).map(|(_, hint)| *hint).collect();
    if hints.is_empty() {
        return;
    }
    for tool in tools.iter_mut().filter(|tool| tool.kind == ToolKind::Bash) {
        let description = tool.spec.description.to_string();
        let hints: Vec<&str> = hints.iter().copied().filter(|hint| !description.contains(hint)).collect();
        if hints.is_empty() {
            continue;
        }
        let hints = hints.join(" ");
        tool.spec.description = match description.split_once(HINTS_BEFORE) {
            Some((before, after)) => format!("{before}{hints} {HINTS_BEFORE}{after}"),
            None => format!("{description} {hints}"),
        }
        .into();
    }
}

/// Each program `installed` names, and what it says of it.
const INSTALLED: [(&str, &str); 2] = [
    ("rg", "ripgrep (rg) is installed: prefer it to grep -r and find for searching code and files."),
    ("jq", "jq is installed: use it to read and filter JSON."),
];

/// The sentence of bash's description the programs' hints go before.
const HINTS_BEFORE: &str = "Run tests and inspect git diff.";

/// Whether `program` is an executable file in one of `path`'s folders.
fn on_path(program: &str, path: &str) -> bool {
    std::env::split_paths(path).filter(|dir| !dir.as_os_str().is_empty()).any(|dir| {
        let Ok(metadata) = std::fs::metadata(dir.join(program)) else { return false };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            metadata.is_file()
        }
    })
}

const SECRETS_HINT: &str = "secrets names secrets granted to you: their variables are set for that command only, and their values are hidden in all output; never print or save a value yourself.";

/// Lets read_file also read the absolute directories `dirs`, and keeps write_file and edit_file
/// from changing anything in them, even where they're inside the tools' working folder. The
/// most specific directory a path is in decides. bash isn't confined.
pub fn read_only(tools: &mut [Tool], dirs: &[PathBuf]) {
    for tool in tools {
        tool.read_only = dirs.to_vec();
    }
}

/// Keeps every file tool out of `dirs`, wherever the agent works: a path in
/// one is refused as written, and so is one whose real place (through links,
/// or its nearest existing parent's) is in one. Bash isn't held to it.
pub fn denied(tools: &mut [Tool], dirs: &[PathBuf]) {
    for tool in tools {
        tool.denied = dirs.to_vec();
    }
}

/// Whether `target` is in one of `dirs`, as written or really.
async fn is_denied(target: &Path, dirs: &[PathBuf]) -> bool {
    if dirs.iter().any(|dir| target.starts_with(dir)) {
        return true;
    }
    let mut existing = target.to_path_buf();
    let real = loop {
        match tokio::fs::canonicalize(&existing).await {
            Ok(real) => break real,
            Err(_) => match existing.parent() {
                Some(parent) => existing = parent.to_path_buf(),
                None => return false,
            },
        }
    };
    for dir in dirs {
        let dir_real = tokio::fs::canonicalize(dir).await.unwrap_or_else(|_| dir.clone());
        if real.starts_with(&dir_real) {
            return true;
        }
    }
    false
}
