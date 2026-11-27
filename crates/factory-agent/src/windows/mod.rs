//! Agents' commands on Windows: what `tools::bash`
//! uses on Windows where it uses process groups and signals on a Mac or
//! Linux.
//!
//! - `shell`: the private Git Bash Claktory ships (never whichever `bash`
//!   is on PATH), its environment, and the wrappers that run a script and
//!   record its exit code.
//! - `job`: a named Job Object per command, standing in for the process
//!   group: every process the command starts is in it, it outlives the
//!   backend (no kill-on-close), and the next backend finds it again by its
//!   name in this Claktory's records.
//! - `spawn`: CreateProcessW with an attribute list, so the command is in
//!   its job from its first instruction (PROC_THREAD_ATTRIBUTE_JOB_LIST) and
//!   inherits only its own handles (PROC_THREAD_ATTRIBUTE_HANDLE_LIST).
//! - `child`: the running shell, as `tokio::process::Child` is elsewhere:
//!   `id`, `wait`, and its output pipes.
//! - `private`: files and folders only this user (and SYSTEM) can open:
//!   mode 0600 and 0700 elsewhere.
//! - `wide`: UTF-16 strings, owned handles and errors for Win32 calls,
//!   factoryd's own (`factoryd::windows`) too.
//! - `guard`, `console`, `positioned`: what `tools::asking`, `login` and
//!   `tools::task_logs` use on Windows instead of /proc, termios and pread.
//! - `system`: when the computer started, when a process started, and how a
//!   process ended.
//!
//! Jobs are how commands are kept track of and stopped, not a security
//! boundary: a command runs as the person, like on a Mac.

pub mod child;
pub mod console;
pub mod guard;
pub mod job;
pub mod positioned;
pub mod private;
pub mod shell;
pub mod spawn;
pub mod system;
pub mod wide;

pub use child::Child;
