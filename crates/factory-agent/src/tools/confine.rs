//! What agents' commands run under, as the program that embeds the agent
//! says at each command's start: a program and its
//! arguments put before the shell, such as a sandbox (`sandbox-exec -p
//! …`), or nothing. One rule for the whole process: factoryd's agents all
//! work as the same person on the same computer.

use std::ffi::OsString;
use std::sync::Arc;

use parking_lot::RwLock;

/// Says, as a command starts, what it runs under: the program and its
/// arguments, or None for nothing.
pub type Confine = Arc<dyn Fn() -> Option<Vec<OsString>> + Send + Sync>;

static CONFINE: RwLock<Option<Confine>> = RwLock::new(None);

/// Sets what every agent command from now on runs under (None: nothing).
pub fn set(confine: Option<Confine>) {
    *CONFINE.write() = confine;
}

/// What a command starting now runs under: empty for nothing.
pub fn prefix() -> Vec<OsString> {
    let confine = CONFINE.read().clone();
    confine.and_then(|confine| confine()).filter(|prefix| !prefix.is_empty()).unwrap_or_default()
}

/// `program` as an agent's command runs it: under what `set` says, if
/// anything.
#[cfg(unix)]
pub fn command(program: &str) -> tokio::process::Command {
    let mut prefix = prefix().into_iter();
    match prefix.next() {
        Some(first) => {
            let mut command = tokio::process::Command::new(first);
            command.args(prefix).arg(program);
            command
        }
        None => tokio::process::Command::new(program),
    }
}
