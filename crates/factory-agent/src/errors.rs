use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentFailure {
    CorruptCredentials { file: String },
    WorkspaceEscape { path: String },
    ReadOnly { path: String },
    /// Inside a folder Claktory keeps from every agent's file tools.
    Private { path: String },
    InvalidEdit { path: String, reason: String },
    BashWrappedInvocation { command: String },
    BashLongSleep { seconds: String },
    BashCopiesRepository { path: String },
}

impl AgentFailure {
    pub fn kind(&self) -> &'static str {
        match self {
            AgentFailure::CorruptCredentials { .. } => "corrupt-credentials",
            AgentFailure::WorkspaceEscape { .. } => "workspace-escape",
            AgentFailure::ReadOnly { .. } => "read-only",
            AgentFailure::Private { .. } => "private",
            AgentFailure::InvalidEdit { .. } => "invalid-edit",
            AgentFailure::BashWrappedInvocation { .. } => "bash-wrapped-invocation",
            AgentFailure::BashLongSleep { .. } => "bash-long-sleep",
            AgentFailure::BashCopiesRepository { .. } => "bash-copies-repository",
        }
    }

    pub fn message(&self) -> String {
        match self {
            AgentFailure::CorruptCredentials { file } => format!("Corrupt credential file at {file}. Delete it and sign in again."),
            AgentFailure::WorkspaceEscape { path } => format!("Path escapes the workspace: {path}"),
            AgentFailure::ReadOnly { path } => format!("Path is read only: {path}. Change files only in your own folder."),
            AgentFailure::Private { path } => format!("{path} is in Claktory's private storage, which no agent's tools read or change."),
            AgentFailure::InvalidEdit { path, reason } => format!("Cannot edit {path}: {reason}"),
            AgentFailure::BashWrappedInvocation { command } => {
                format!("command is \"{command}\", not a script. Put the script text itself in command.")
            }
            AgentFailure::BashLongSleep { seconds } => format!(
                "Blocked: sleep {seconds}. Claktory wakes you with an event when a background task ends or a message arrives, so there's nothing to wait for. To wait for a condition, add a monitor (check) or run an until-loop with run_in_background. Don't chain shorter sleeps to get around this."
            ),
            AgentFailure::BashCopiesRepository { path } => format!(
                "Blocked: this copies the whole repository at {path}, build output included, which takes many minutes. Don't copy the repository. If you need a checkout of your own, ask for Claktory's Own checkout extension; to check what's committed, export it with `git archive HEAD | tar -x -C <dir>`."
            ),
        }
    }
}

#[derive(Debug)]
pub struct AgentError {
    pub failure: AgentFailure,
    pub cause: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl AgentError {
    pub fn new(failure: AgentFailure) -> AgentError {
        AgentError { failure, cause: None }
    }

    pub fn with_cause(failure: AgentFailure, cause: impl std::error::Error + Send + Sync + 'static) -> AgentError {
        AgentError { failure, cause: Some(Box::new(cause)) }
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.failure.message())
    }
}

impl std::error::Error for AgentError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_deref().map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

const NODE_ERRORS: &[(i32, &str, &str)] = &[
    (libc::ENOENT, "ENOENT", "no such file or directory"),
    (libc::EISDIR, "EISDIR", "illegal operation on a directory"),
    (libc::ENOTDIR, "ENOTDIR", "not a directory"),
    (libc::EACCES, "EACCES", "permission denied"),
    (libc::EEXIST, "EEXIST", "file already exists"),
    (libc::EPERM, "EPERM", "operation not permitted"),
    (libc::ENOTEMPTY, "ENOTEMPTY", "directory not empty"),
    (libc::ELOOP, "ELOOP", "too many symbolic links encountered"),
    (libc::EINVAL, "EINVAL", "invalid argument"),
    (libc::ENAMETOOLONG, "ENAMETOOLONG", "name too long"),
    (libc::EMFILE, "EMFILE", "too many open files"),
    (libc::EBUSY, "EBUSY", "resource busy or locked"),
    (libc::EXDEV, "EXDEV", "cross-device link not permitted"),
    (libc::EROFS, "EROFS", "read-only file system"),
    (libc::ENOSPC, "ENOSPC", "no space left on device"),
    (libc::EIO, "EIO", "i/o error"),
    (libc::EFBIG, "EFBIG", "file too large"),
];

/// An I/O failure printed the way Node prints it, because the model reads the text.
#[derive(Debug)]
pub struct FsError {
    pub code: &'static str,
    pub syscall: &'static str,
    pub path: Option<PathBuf>,
    pub dest: Option<PathBuf>,
    pub source: std::io::Error,
}

impl FsError {
    pub fn new(source: std::io::Error, syscall: &'static str, path: Option<&Path>, dest: Option<&Path>) -> FsError {
        let code =
            source.raw_os_error().and_then(|errno| NODE_ERRORS.iter().find(|entry| entry.0 == errno)).map_or("UNKNOWN", |entry| entry.1);
        FsError { code, syscall, path: path.map(Path::to_path_buf), dest: dest.map(Path::to_path_buf), source }
    }

    pub fn is_not_found(&self) -> bool {
        self.source.kind() == std::io::ErrorKind::NotFound
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let description =
            NODE_ERRORS.iter().find(|entry| entry.1 == self.code).map_or_else(|| self.source.to_string(), |entry| entry.2.to_owned());
        write!(f, "{}: {description}, {}", self.code, self.syscall)?;
        if let Some(path) = &self.path {
            write!(f, " '{}'", path.display())?;
        }
        if let Some(dest) = &self.dest {
            write!(f, " -> '{}'", dest.display())?;
        }
        Ok(())
    }
}

impl std::error::Error for FsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Agent(#[from] AgentError),
    #[error("{0}")]
    Fs(#[from] FsError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("This operation was aborted")]
    Aborted,
    #[error("{message}")]
    Models { code: &'static str, message: String },
    #[error("Compaction failed: {0}")]
    Compaction(String),
    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn failure(&self) -> Option<&AgentFailure> {
        match self {
            Error::Agent(error) => Some(&error.failure),
            Error::Fs(_)
            | Error::Io(_)
            | Error::Json(_)
            | Error::Aborted
            | Error::Models { .. }
            | Error::Compaction(_)
            | Error::Other(_) => None,
        }
    }

    pub fn fs(source: std::io::Error, syscall: &'static str, path: &Path) -> Error {
        FsError::new(source, syscall, Some(path), None).into()
    }

    pub fn fs_rename(source: std::io::Error, from: &Path, to: &Path) -> Error {
        FsError::new(source, "rename", Some(from), Some(to)).into()
    }
}

/// `Error::Models`' code when the provider refused the credentials: a key it
/// doesn't accept, or a sign-in whose token it won't refresh.
pub const REFUSED: &str = "refused";

/// `Error::Models`' code when the sign-ins are locked away with the
/// person's secrets until they open (`credentials::locked_error`).
pub const LOCKED: &str = "locked";

impl Error {
    /// Whether the provider refused the credentials (`REFUSED`).
    pub fn is_refused(&self) -> bool {
        matches!(self, Error::Models { code: REFUSED, .. })
    }

    /// Whether the sign-ins are locked away (`LOCKED`).
    pub fn is_locked(&self) -> bool {
        matches!(self, Error::Models { code: LOCKED, .. })
    }
}

/// Pi's `ModelsError`: the cause's text joins the message because callers show `message` alone.
pub fn models_error(code: &'static str, message: impl Into<String>, cause: Option<&Error>) -> Error {
    let message = message.into();
    let message = match cause.map(ToString::to_string) {
        Some(detail) if !detail.trim().is_empty() && !message.contains(detail.trim()) => format!("{message}: {}", detail.trim()),
        _ => message,
    };
    Error::Models { code, message }
}
