#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
#[cfg(unix)]
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;
#[cfg(unix)]
use tokio::process::{Child, Command};
use tokio::time::{Instant, Sleep, sleep};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::errors::{AgentError, AgentFailure, Error};
use crate::secrets::{Redaction, Redactor, SAVE_MOST, SaveAs, SecretUse, SecretVariable, Secrets, StreamRedactor};
use crate::text::{utf16_head, utf16_len, utf16_tail};
use crate::tools::asking::Guard;
use crate::tools::task_logs;
use crate::tools::{MAX_OUTPUT, Update, checks};
use crate::types::ToolResult;
#[cfg(windows)]
use crate::windows::{Child, shell::Shell};

// The two signals a stop sends; on Windows both end the command's job.
#[cfg(unix)]
use libc::{SIGKILL, SIGTERM};
#[cfg(windows)]
const SIGTERM: i32 = 15;
#[cfg(windows)]
const SIGKILL: i32 = 9;

/// Output a command may write through pipes (with no background folder to
/// write it to) before it's stopped.
pub const OUTPUT_BYTES: usize = 8 * 1024 * 1024;
/// Output a command may write to its log before it's stopped.
pub const LOG_BYTES: u64 = 5 * 1024 * 1024 * 1024;
/// How long a command runs in the foreground, in seconds, unless it says
/// otherwise; then it moves to the background.
pub const DEFAULT_TIMEOUT_SECS: f64 = 120.0;
/// The longest a command may run in the foreground, in seconds.
pub const MAX_TIMEOUT_SECS: f64 = 600.0;
/// How long a background command may run, in seconds, unless it says otherwise.
pub const BACKGROUND_TIMEOUT_SECS: f64 = 1800.0;
/// The longest `timeout` a background command may ask for, in seconds.
pub const MAX_BACKGROUND_SECS: f64 = 7200.0;
/// How long a command that moved to the background may run on after the move.
const MOVED_LIMIT_SECS: f64 = 1800.0;
/// Output up to this many characters comes back whole; past it, the result
/// has its start and end, and the path of the whole output.
pub const INLINE_OUTPUT: usize = 30_000;
const PREVIEW_HEAD: usize = 2_000;
const PREVIEW_TAIL: usize = 10_000;
/// A background task's status when it ran past its time limit.
const TIME_LIMIT: &str = "stopped at its time limit";
/// The status of a task the computer's restart ended.
const RESTARTED: &str = "the computer restarted";
/// The status of a task that ended unwatched, without saying how.
const LOST: &str = "ended while Claktory was stopped";
/// How often a running command's log is read for its output.
const POLL: Duration = Duration::from_millis(100);

/// How much of a background task's output its finishing event carries.
const FINISHED_TAIL: usize = 4_000;

/// Lets bash run commands in the background. Each task writes its output
/// straight to `<dir>/<id>.log` and its exit code to `<dir>/<id>.status`, so
/// it runs on even if the program that started it stops. `started` and
/// `finished` tell that program (Factory records tasks and sends the agent an
/// event when one ends).
#[derive(Clone)]
pub struct Background {
    pub dir: PathBuf,
    /// Variables every command gets, foreground or background, before its
    /// secrets' (Factory sets FACTORY_AGENT_ID to the agent's id).
    pub environment: Vec<(String, String)>,
    pub started: Arc<dyn Fn(&Task) + Send + Sync>,
    pub finished: Arc<dyn Fn(Finished) + Send + Sync>,
}

impl Background {
    /// The agent whose commands these are (FACTORY_AGENT_ID), or ''.
    fn agent(&self) -> &str {
        self.environment.iter().find(|(name, _)| name == "FACTORY_AGENT_ID").map_or("", |(_, value)| value.as_str())
    }

    /// Its own variables, then `secrets` (so a secret's variable wins).
    fn with(&self, secrets: Vec<(String, Zeroizing<String>)>) -> Vec<(String, Zeroizing<String>)> {
        let own = self.environment.iter().map(|(name, value)| (name.clone(), Zeroizing::new(value.clone())));
        own.chain(secrets).collect()
    }
}

/// A command running in the background.
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: String,
    pub command: String,
    /// Its process group.
    pub pid: u32,
    pub log: PathBuf,
    /// When it started, in ms since 1970; 0 if unknown.
    pub started: u64,
    /// The folder it started in; empty if unknown.
    pub folder: PathBuf,
}

impl Task {
    fn status_path(&self) -> PathBuf {
        self.log.with_extension("status")
    }

    /// The folder of the files its secrets put there, if any.
    fn secrets_dir(&self) -> PathBuf {
        self.log.with_extension("secrets")
    }

    /// Waits for a task this process no longer watches (it was started
    /// before a restart): until it records its exit code, or its process
    /// group is gone because it was killed. `redaction` hides secrets in
    /// the end of its output.
    /// A task started before the computer last started was ended by its
    /// restart, whatever process has its pid now.
    pub async fn wait_detached(self, redaction: &Redaction) -> Finished {
        if self.recorded_exit().is_none() && started_before_boot(self.started) {
            let mut done = self.finished(RESTARTED.to_owned(), &redaction.current());
            done.seconds = None;
            return done;
        }
        let mut measured = Instant::now();
        while self.recorded_exit().is_none() && group_alive(self.pid) {
            sleep(Duration::from_secs(1)).await;
            if measured.elapsed() >= task_logs::CHECK_EVERY {
                measured = Instant::now();
                capped(&self).await;
            }
        }
        self.finished("stopped".to_owned(), &redaction.current())
    }

    /// Whether this task, which this process no longer watches (it was
    /// started before a restart), ended unwatched without recording how:
    /// the computer's restart ended it, or its process group is gone. Its
    /// agent is told of such tasks together, once (friend bug #44).
    pub fn lost(&self) -> bool {
        self.recorded_exit().is_none() && (started_before_boot(self.started) || !group_alive(self.pid))
    }

    /// A lost task's end, which no event tells: its status says why, and
    /// its secrets' files go.
    pub fn lost_end(self, redaction: &Redaction) -> Finished {
        let why = if started_before_boot(self.started) { RESTARTED } else { LOST };
        let mut done = self.finished(why.to_owned(), &redaction.current());
        done.seconds = None;
        done
    }

    fn recorded_exit(&self) -> Option<i32> {
        std::fs::read_to_string(self.status_path()).ok()?.trim().parse().ok()
    }

    /// How it ended, from the exit code it recorded; `otherwise` when it
    /// recorded none (it was killed). Its secrets' files go.
    fn finished(self, otherwise: String, redactor: &Redactor) -> Finished {
        remove_secret_files(&self.secrets_dir());
        // Written past its cap between two checks: cut now that it's done.
        if let Err(error) = task_logs::cap_finished(&self.log, self.pid)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(task = %self.id, %error, "could not cut a task's log");
        }
        let status = match self.recorded_exit() {
            Some(code) if code > 128 => ended_text(&signal_name(code - 128)),
            Some(code) => format!("exit {code}"),
            None => otherwise,
        };
        let tail = log_tail(&self.log, redactor);
        let seconds = (self.started > 0).then(|| crate::clock::now_ms().saturating_sub(self.started) as f64 / 1000.0);
        Finished { id: self.id, command: self.command, log: self.log, status, tail, seconds }
    }
}

/// A background task that ended.
#[derive(Clone, Debug)]
pub struct Finished {
    pub id: String,
    pub command: String,
    pub log: PathBuf,
    /// "exit 0", "signal SIGTERM", "timed out"…
    pub status: String,
    /// The end of its output.
    pub tail: String,
    /// How long it ran, in seconds, when known.
    pub seconds: Option<f64>,
}

impl Finished {
    /// "exit 0, 4.2s": its status and how long it ran.
    fn ended(&self) -> String {
        match self.seconds {
            Some(seconds) => format!("{}, {}", self.status, took(seconds)),
            None => self.status.clone(),
        }
    }

    /// One line for people.
    pub fn summary(&self) -> String {
        let command = self.command.lines().next().unwrap_or_default();
        let short = utf16_head(command, 80);
        let more = if short.len() < self.command.trim_end().len() { "…" } else { "" };
        format!("{} finished ({}): {short}{more}", self.id, self.ended())
    }

    /// What the agent is told.
    pub fn text(&self) -> String {
        let limit = if self.status.starts_with(TIME_LIMIT) {
            "\nIt ran past its time limit, so it was stopped. Run it again (with run_in_background and a longer timeout, up to 7200 seconds) only if the work still needs it; better, find out why it's so slow and fix that."
        } else if self.status == RESTARTED {
            "\nThe computer restarted while it ran, which ended it. Start it again if the work still needs it."
        } else {
            ""
        };
        format!(
            "Background {} finished ({}).{limit}\nCommand: {}\nFull output: {}\n\nEnd of its output:\n{}",
            self.id,
            self.ended(),
            self.command,
            self.log.display(),
            self.tail
        )
    }
}

/// "0.4s", "42.0s", "3m 07s", "1h 02m": how long something took.
pub fn took(seconds: f64) -> String {
    let whole = seconds.max(0.0).round() as u64;
    match seconds {
        seconds if seconds < 60.0 => format!("{:.1}s", seconds.max(0.0)),
        _ if whole < 3600 => format!("{}m {:02}s", whole / 60, whole % 60),
        _ => format!("{}h {:02}m", whole / 3600, whole % 3600 / 60),
    }
}

/// Runs the task's script ($1) and records its exit code in $2.
#[cfg(unix)]
const TASK_WRAPPER: &str = r#"bash --noprofile --norc -e -o pipefail -c "$1"; echo $? > "$2""#;

/// As TASK_WRAPPER, for a command with secrets: its output goes to the log
/// through the filter ($3 and its arguments), which hides their values; the
/// values the filter gets aren't the command's.
#[cfg(unix)]
const FILTERED_TASK_WRAPPER: &str = concat!(
    r#"( unset "#,
    "CLAKTORY_REDACT",
    r#"; exec bash --noprofile --norc -e -o pipefail -c "$1" ) 2>&1 | "${@:3}"; echo "${PIPESTATUS[0]}" > "$2""#
);

/// When this computer last started, in ms since 1970; None if unknown.
pub fn boot_time_ms() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut boot = libc::timeval { tv_sec: 0, tv_usec: 0 };
        let mut size = std::mem::size_of::<libc::timeval>();
        // SAFETY: kern.boottime is a timeval, and size says how big ours is.
        let result =
            unsafe { libc::sysctlbyname(c"kern.boottime".as_ptr(), (&raw mut boot).cast(), &raw mut size, std::ptr::null_mut(), 0) };
        (result == 0 && boot.tv_sec > 0).then(|| boot.tv_sec as u64 * 1000 + boot.tv_usec as u64 / 1000)
    }
    #[cfg(windows)]
    {
        crate::windows::system::boot_time_ms()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        let seconds: u64 = stat.lines().find_map(|line| line.strip_prefix("btime "))?.trim().parse().ok()?;
        Some(seconds * 1000)
    }
}

/// Whether something that started at `started` (ms since 1970; 0 if
/// unknown) began before the computer last started, so a restart ended it.
pub fn started_before_boot(started: u64) -> bool {
    started > 0 && boot_time_ms().is_some_and(|boot| started < boot)
}

/// Whether any process of the group is still there.
pub fn group_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        crate::windows::job::alive(pid)
    }
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process group exists.
        let result = unsafe { libc::kill(-(pid as i32), 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

/// Cuts the middle of a running task's log once it's past its cap
/// (`task_logs`), off the async threads.
async fn capped(task: &Task) {
    let (log, pid) = (task.log.clone(), task.pid);
    match tokio::task::spawn_blocking(move || task_logs::cap(&log, pid)).await {
        Ok(Ok(true)) => tracing::info!(task = %task.id, log = %task.log.display(), "cut the middle of a task's log"),
        Ok(Err(error)) if error.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(task = %task.id, %error, "could not cut a task's log");
        }
        _ => {}
    }
}

/// The last FINISHED_TAIL characters of a log, reading only its end, with
/// secrets hidden (before it's cut, so a value the cut would split is too).
fn log_tail(path: &Path, redactor: &Redactor) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return String::new() };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let start = length.saturating_sub(FINISHED_TAIL as u64 * 4 + redactor.longest() as u64);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(start)).and_then(|_| file.read_to_end(&mut bytes)).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    utf16_tail(&redactor.redact(&text), FINISHED_TAIL).to_owned()
}

/// Removes the folder of a command's secret files, if it has one.
fn remove_secret_files(dir: &Path) {
    match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            tracing::warn!(dir = %dir.display(), %error, "could not remove a command's secret files");
        }
        _ => {}
    }
}

/// The secrets a command runs with.
struct Injected<'a> {
    secrets: Option<&'a Secrets>,
    /// The names it asked for.
    names: Vec<String>,
    variables: Vec<SecretVariable>,
    /// Hides every value there is, its own included.
    redactor: Arc<Redactor>,
}

impl Injected<'_> {
    /// Its environment variables; file secrets are written into `dir`
    /// (mode 0700, the files 0600), which the caller removes when the
    /// command ends.
    fn environment(&self, dir: &Path) -> std::io::Result<Vec<(String, Zeroizing<String>)>> {
        crate::secrets::command_environment(&self.variables, dir, || {
            remove_secret_files(dir);
            Ok(())
        })
    }

    /// The filter a command's log goes through, and the values it hides:
    /// for a command with secrets, when there's a filter to run.
    fn filter(&self) -> Option<(&[std::ffi::OsString], Zeroizing<String>)> {
        let secrets = self.secrets?;
        (!self.variables.is_empty() && !secrets.filter.is_empty())
            .then(|| (secrets.filter.as_slice(), crate::secrets::filter_values(&self.variables)))
    }

    /// Tells the program running the agent that `command` ran with them.
    fn used(&self, command: &str, task: Option<String>, started: u64) {
        if let Some(secrets) = self.secrets
            && !self.names.is_empty()
        {
            (secrets.used)(SecretUse {
                agent: secrets.agent.clone(),
                secrets: self.names.clone(),
                command: command.to_owned(),
                task,
                started,
            });
        }
    }
}

/// Removes a command's secret files when it goes, unless it was kept
/// (its command runs on as a background task, which removes them).
struct SecretFiles(Option<PathBuf>);

impl SecretFiles {
    fn keep(&mut self) {
        self.0 = None;
    }
}

impl Drop for SecretFiles {
    fn drop(&mut self) {
        if let Some(dir) = &self.0 {
            remove_secret_files(dir);
        }
    }
}

/// The secrets the call asks for, resolved, and what hides them.
async fn inject<'a>(args: &BashArgs, secrets: Option<&'a Secrets>) -> Result<Injected<'a>, Error> {
    let mut names: Vec<String> = Vec::new();
    for name in args.secrets.iter().flatten() {
        let name = name.trim();
        if !name.is_empty() && !names.iter().any(|seen| seen == name) {
            names.push(name.to_owned());
        }
    }
    let current = secrets.map(|secrets| secrets.redaction.current()).unwrap_or_default();
    if names.is_empty() {
        return Ok(Injected { secrets, names, variables: Vec::new(), redactor: current });
    }
    let Some(found) = secrets else { return Err(Error::Other("Secrets aren't available here.".into())) };
    let variables = (found.resolve)(found.agent.clone(), names.clone()).await.map_err(Error::Other)?;
    let redactor = Arc::new(current.with(&variables));
    Ok(Injected { secrets, names, variables, redactor })
}

const WRAPPED_INVOCATIONS: [&str; 8] = ["bash", "sh", "bash -c", "bash -lc", "sh -c", "sh -lc", "/bin/bash -c", "/bin/sh -c"];

#[cfg(unix)]
const SIGNALS: &[(i32, &str)] = &[
    (libc::SIGHUP, "SIGHUP"),
    (libc::SIGINT, "SIGINT"),
    (libc::SIGQUIT, "SIGQUIT"),
    (libc::SIGILL, "SIGILL"),
    (libc::SIGTRAP, "SIGTRAP"),
    (libc::SIGABRT, "SIGABRT"),
    (libc::SIGBUS, "SIGBUS"),
    (libc::SIGFPE, "SIGFPE"),
    (libc::SIGKILL, "SIGKILL"),
    (libc::SIGUSR1, "SIGUSR1"),
    (libc::SIGSEGV, "SIGSEGV"),
    (libc::SIGUSR2, "SIGUSR2"),
    (libc::SIGPIPE, "SIGPIPE"),
    (libc::SIGALRM, "SIGALRM"),
    (libc::SIGTERM, "SIGTERM"),
    (libc::SIGCHLD, "SIGCHLD"),
    (libc::SIGXCPU, "SIGXCPU"),
    (libc::SIGXFSZ, "SIGXFSZ"),
    (libc::SIGSYS, "SIGSYS"),
];

fn signal_name(signal: i32) -> String {
    #[cfg(unix)]
    {
        SIGNALS.iter().find(|entry| entry.0 == signal).map_or_else(|| format!("SIG{signal}"), |entry| entry.1.to_owned())
    }
    // Git Bash reports a signal from its own `kill` as 128+n too.
    #[cfg(windows)]
    {
        format!("SIG{signal}")
    }
}

/// How a process that ended other than by exiting ended: its signal's name
/// on a Mac or Linux; on Windows, what ended it ("stopped", "access
/// violation").
fn exit_signal(status: &std::process::ExitStatus) -> Option<String> {
    #[cfg(unix)]
    {
        status.signal().map(signal_name)
    }
    #[cfg(windows)]
    {
        crate::windows::system::ended_by(status).map(str::to_owned)
    }
}

/// "signal SIGTERM"; on Windows, where nothing ends by a signal but Git
/// Bash's own, what ended it ("stopped").
fn ended_text(signal: &str) -> String {
    if signal.starts_with("SIG") { format!("signal {signal}") } else { signal.to_owned() }
}

/// Node's `StringDecoder("utf8")`: incomplete trailing sequences wait for the next chunk.
#[derive(Debug, Default)]
pub struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub fn write(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(text) => {
                    out.push_str(text);
                    self.pending.clear();
                    return out;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.pending[..valid]).unwrap_or_default());
                    match error.error_len() {
                        Some(bad) => {
                            out.push('\u{fffd}');
                            self.pending.drain(..valid + bad);
                        }
                        None => {
                            self.pending.drain(..valid);
                            return out;
                        }
                    }
                }
            }
        }
    }

    pub fn end(&mut self) -> String {
        if self.pending.is_empty() {
            String::new()
        } else {
            self.pending.clear();
            "\u{fffd}".to_owned()
        }
    }
}

#[derive(Deserialize)]
struct BashArgs {
    command: String,
    timeout: Option<f64>,
    #[serde(default)]
    run_in_background: Option<bool>,
    /// Names of secrets to set in the command's environment.
    #[serde(default)]
    secrets: Option<Vec<String>>,
    /// Saves the command's whole output as a new secret instead of
    /// showing it.
    #[serde(default)]
    save_as: Option<SaveAs>,
}

struct Output<'a> {
    tail: String,
    characters: usize,
    output_bytes: usize,
    stopped: Option<&'static str>,
    update: Update<'a>,
    stdout: StreamRedactor,
    stderr: StreamRedactor,
}

impl Output<'_> {
    /// Takes the stream's next text; `end`: it's the last.
    fn consume(&mut self, text: &str, stream: &str, end: bool) {
        let redactor = if stream == "stderr" { &mut self.stderr } else { &mut self.stdout };
        let mut text = redactor.push(text);
        if end {
            text.push_str(&redactor.finish());
        }
        self.characters += utf16_len(&text);
        let joined = format!("{}{text}", self.tail);
        self.tail = utf16_tail(&joined, MAX_OUTPUT).to_owned();
        (self.update)(ToolResult::text(text, json!({"stream": stream})));
    }

    fn chunk(&mut self, decoder: &mut Utf8Decoder, bytes: &[u8], stream: &str) -> bool {
        let remaining = OUTPUT_BYTES.saturating_sub(self.output_bytes);
        self.output_bytes += bytes.len();
        if remaining > 0 {
            let text = decoder.write(&bytes[..remaining.min(bytes.len())]);
            self.consume(&text, stream, false);
        }
        self.output_bytes > OUTPUT_BYTES
    }
}

/// How long a bash call with these arguments may run before it moves to the
/// background (or, without a background, is stopped): its `timeout`, or the
/// default, at most `MAX_TIMEOUT_SECS`. None for a background command, whose
/// call returns at once.
pub fn call_timeout(args: &Value) -> Option<Duration> {
    let args: BashArgs = serde_json::from_value(args.clone()).ok()?;
    if args.run_in_background == Some(true) {
        return None;
    }
    Duration::try_from_secs_f64(foreground_timeout(args.timeout)).ok()
}

fn foreground_timeout(timeout: Option<f64>) -> f64 {
    timeout.unwrap_or(DEFAULT_TIMEOUT_SECS).min(MAX_TIMEOUT_SECS)
}

/// "process group 4242" on a Mac or Linux; on Windows, where the task is a
/// job named by its shell's pid, "process 4242".
fn group_text(pid: u32) -> String {
    if cfg!(windows) { format!("process {pid}") } else { format!("process group {pid}") }
}

/// The command an agent stops a task with itself: the whole process group,
/// or on Windows the shell's process tree (Git Bash's own `kill` knows only
/// its own pids).
fn stop_hint(pid: u32) -> String {
    if cfg!(windows) { format!("taskkill //F //T //PID {pid}") } else { format!("kill -- -{pid}") }
}

/// Stops a background task: its process group gets SIGTERM, and SIGKILL
/// half a second later if it's still there.
pub fn stop_group(pid: u32) {
    kill_group(pid, SIGTERM);
    tokio::spawn(async move {
        sleep(Duration::from_millis(500)).await;
        if group_alive(pid) {
            kill_group(pid, SIGKILL);
        }
    });
}

fn kill_group(pid: u32, signal: i32) {
    // Windows has no gentler stop for a whole job: either ends it.
    #[cfg(windows)]
    {
        let _ = signal;
        crate::windows::job::stop(pid);
    }
    // SAFETY: a plain syscall on the process group this tool created; ESRCH after exit is expected.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), signal);
    }
}

fn stop(output: &mut Output, pid: u32, reason: &'static str, kill_timer: &mut Pin<&mut Sleep>, kill_armed: &mut bool) {
    if output.stopped.is_some() {
        return;
    }
    output.stopped = Some(reason);
    kill_group(pid, SIGTERM);
    kill_timer.as_mut().reset(Instant::now() + Duration::from_millis(500));
    *kill_armed = true;
}

/// Commands run with the permissions of the process that embeds the agent; the agent is not a sandbox.
/// `folders` are the agent's own folder and the others it may read: a
/// command that copies the repository any of them is in is refused.
/// `secrets` resolves the secrets the call names and hides their values.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    cwd: &Path,
    folders: &[&Path],
    args: &Value,
    cancel: &CancellationToken,
    detach: &CancellationToken,
    update: Update<'_>,
    background: Option<&Background>,
    secrets: Option<&Secrets>,
) -> Result<ToolResult, Error> {
    let args: BashArgs = serde_json::from_value(args.clone())?;
    if cancel.is_cancelled() {
        return Err(Error::Aborted);
    }
    let collapsed = args.command.split_whitespace().collect::<Vec<_>>().join(" ");
    let invocation = collapsed.strip_suffix(" ''").or_else(|| collapsed.strip_suffix(" \"\"")).unwrap_or(&collapsed);
    if WRAPPED_INVOCATIONS.contains(&invocation) {
        return Err(AgentError::new(AgentFailure::BashWrappedInvocation { command: invocation.to_owned() }).into());
    }
    if args.run_in_background != Some(true)
        && let Some(failure) = checks::long_sleep(&args.command)
    {
        return Err(AgentError::new(failure).into());
    }
    // Asking PowerShell for an administrator's rights (best effort; the
    // programs that do only that are stopped as they run, by the guard).
    #[cfg(windows)]
    if let Some(program) = crate::windows::guard::asking_in_text(&args.command) {
        return Err(Error::Other(crate::tools::asking::told(program)));
    }
    let mut roots: Vec<PathBuf> = folders.iter().filter_map(|folder| checks::repository_root(folder)).collect();
    roots.sort();
    roots.dedup();
    if let Some(failure) = checks::copies_repository(&args.command, cwd, &roots) {
        return Err(AgentError::new(failure).into());
    }
    if args.save_as.is_some() {
        return run_saving(cwd, args, cancel, background, secrets).await;
    }
    if args.run_in_background == Some(true) && background.is_none() {
        return Err(Error::Other("Background commands aren't available here.".into()));
    }
    let injected = inject(&args, secrets).await?;
    if let Some(background) = background {
        if args.run_in_background == Some(true) {
            return start_background(cwd, args, background, &injected).await;
        }
        // macOS refusing a protected folder says so (permissions.md P2).
        return run_logged(cwd, args, cancel, detach, update, background, &injected).await.map(|result| super::protected::noted(result, cwd));
    }
    let began = std::time::Instant::now();
    let started = crate::clock::now_ms();
    let files = SecretFiles(Some(std::env::temp_dir().join(format!("claktory-secrets-{}", uuid::Uuid::new_v4()))));
    let environment = injected.environment(files.0.as_deref().unwrap_or(cwd))?;
    let (child, pid) = spawn(cwd, &args.command, &environment)?;
    let guard = Guard::watch(pid, "", None);
    let ended = collect(child, pid, Some(foreground_timeout(args.timeout)), cancel, update, &injected.redactor).await?;
    drop(files);
    injected.used(&args.command, None, started);
    let truncated = if ended.characters > utf16_len(&ended.tail) { "[Earlier output truncated]\n" } else { "" };
    let said = guard.said();
    let stopped = if said.is_empty() { String::new() } else { format!("\n{said}") };
    let text = format!("{truncated}{}{stopped}\n[{}, {}]", ended.tail, ended.state, took(began.elapsed().as_secs_f64()));
    let result = ToolResult::text(
        text,
        json!({"exitCode": ended.exit_code, "exitSignal": ended.exit_signal, "stopped": ended.stopped, "characters": ended.characters, "outputBytes": ended.output_bytes}),
    );
    Ok(super::protected::noted(result, cwd))
}

/// Runs a command whose whole output (stdout) is saved as a new secret
/// (`save_as`, Fugo's save-as-secret.md): for a key made with a secret the
/// agent holds, so the key never passes through the model, the transcript
/// or a log. Everything that can refuse the save is checked before the
/// command runs, so a key made at a service isn't lost to a refusal after;
/// the rest (the value is another secret's, the output's empty or too big)
/// only after. It runs in the foreground only, to its end or its timeout,
/// never moving to the background: a log would hold the output. stdout is
/// kept apart and never shown; stderr comes back once the command ended,
/// with the new value hidden too.
async fn run_saving(
    cwd: &Path,
    args: BashArgs,
    cancel: &CancellationToken,
    background: Option<&Background>,
    secrets: Option<&Secrets>,
) -> Result<ToolResult, Error> {
    let Some(save_as) = args.save_as.clone() else { return Err(Error::Other("Nothing to save.".into())) };
    if args.run_in_background == Some(true) {
        return Err(Error::Other(
            "save_as keeps a command's output out of every log, so it runs in the foreground only: leave out run_in_background. The command didn't run.".into(),
        ));
    }
    let Some((found, save)) = secrets.and_then(|found| Some((found, found.save.as_ref()?))) else {
        return Err(Error::Other("Saving a command's output as a secret isn't available here. The command didn't run.".into()));
    };
    let injected = inject(&args, secrets).await?;
    (save.check)(found.agent.clone(), save_as.clone(), injected.names.clone()).await.map_err(Error::Other)?;
    let began = std::time::Instant::now();
    let started = crate::clock::now_ms();
    let files = SecretFiles(Some(std::env::temp_dir().join(format!("claktory-secrets-{}", uuid::Uuid::new_v4()))));
    let own = injected.environment(files.0.as_deref().unwrap_or(cwd))?;
    let environment = match background {
        Some(background) => background.with(own),
        None => own,
    };
    let (mut child, pid) = spawn(cwd, &args.command, &environment)?;
    let guard = Guard::watch(pid, &found.agent, None);
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    // One byte past the most a secret holds says it's too much; past
    // OUTPUT_BYTES in all, the command is stopped.
    let mut kept = Zeroizing::new(Vec::with_capacity(SAVE_MOST + 1));
    let mut printed = 0usize;
    let mut said = Vec::new();
    let (mut out_buf, mut err_buf) = (Zeroizing::new(vec![0u8; 65_536]), vec![0u8; 65_536]);
    let timer = sleep(Duration::from_secs_f64(foreground_timeout(args.timeout)));
    tokio::pin!(timer);
    let kill_timer = sleep(Duration::from_secs(0));
    tokio::pin!(kill_timer);
    let mut kill_armed = false;
    let mut stopped: Option<&'static str> = None;
    let halt = |reason: &'static str, stopped: &mut Option<&'static str>, kill_timer: &mut Pin<&mut Sleep>, kill_armed: &mut bool| {
        if stopped.is_none() {
            *stopped = Some(reason);
            kill_group(pid, SIGTERM);
            kill_timer.as_mut().reset(Instant::now() + Duration::from_millis(500));
            *kill_armed = true;
        }
    };
    let mut exit = None;
    while stdout.is_some() || stderr.is_some() || exit.is_none() {
        tokio::select! {
            biased;
            () = &mut timer, if stopped.is_none() => halt("timed out", &mut stopped, &mut kill_timer, &mut kill_armed),
            () = cancel.cancelled(), if stopped.is_none() => halt("aborted", &mut stopped, &mut kill_timer, &mut kill_armed),
            () = &mut kill_timer, if kill_armed => {
                kill_armed = false;
                kill_group(pid, SIGKILL);
            }
            read = async { match stdout.as_mut() { Some(pipe) => pipe.read(&mut out_buf).await, None => std::future::pending().await } }, if stdout.is_some() => {
                match read {
                    Ok(n) if n > 0 => {
                        printed += n;
                        let room = (SAVE_MOST + 1).saturating_sub(kept.len());
                        kept.extend_from_slice(&out_buf[..n.min(room)]);
                        if printed + said.len() > OUTPUT_BYTES {
                            halt("output limit exceeded", &mut stopped, &mut kill_timer, &mut kill_armed);
                        }
                    }
                    _ => stdout = None,
                }
            }
            read = async { match stderr.as_mut() { Some(pipe) => pipe.read(&mut err_buf).await, None => std::future::pending().await } }, if stderr.is_some() => {
                match read {
                    Ok(n) if n > 0 => {
                        let room = OUTPUT_BYTES.saturating_sub(said.len());
                        said.extend_from_slice(&err_buf[..n.min(room)]);
                        if n > room || printed + said.len() > OUTPUT_BYTES {
                            halt("output limit exceeded", &mut stopped, &mut kill_timer, &mut kill_armed);
                        }
                    }
                    _ => stderr = None,
                }
            }
            status = child.wait(), if exit.is_none() => exit = Some(status?),
        }
    }
    if stopped.is_some() {
        kill_group(pid, SIGKILL);
    }
    drop(files);
    injected.used(&args.command, None, started);
    let status = exit.unwrap_or_default();
    let exit_signal = exit_signal(&status);
    // On Windows a process that ended other than by exiting has a code too.
    let exit_code = if exit_signal.is_some() { None } else { status.code() };
    let state = match (stopped, &exit_signal, exit_code) {
        (Some(reason), _, _) => reason.to_owned(),
        (None, Some(signal), _) => ended_text(signal),
        (None, None, code) => format!("exit {}", code.unwrap_or_default()),
    };
    // The output is hidden in what the command said, saved or not: a key
    // that wasn't saved is still a key. Output past the most a secret
    // holds is no key, and isn't: a long run of the same bytes would cost
    // the redactor minutes to take in. Nothing said, nothing to hide in.
    let hidden = (kept.len() <= SAVE_MOST && !said.is_empty()).then(|| saved_variable(&save_as, &kept));
    let nothing = "Nothing was saved: if the command made a key at a service, revoke it there, then run it again.";
    let saved = if let Some(reason) = stopped {
        let what = match reason {
            "timed out" => "The command timed out.",
            "aborted" => "The command was stopped.",
            _ => "The command printed too much, so it was stopped.",
        };
        Err(format!("{what} {nothing}"))
    } else if exit_code != Some(0) {
        Err(format!("The command failed ({state}). {nothing}"))
    } else if kept.len() > SAVE_MOST {
        Err(format!("The command printed more than a secret holds ({} KiB). {nothing}", SAVE_MOST / 1024))
    } else if kept.iter().all(u8::is_ascii_whitespace) {
        Err("The command printed nothing to save, so nothing was saved.".to_owned())
    } else {
        let bytes = kept.len();
        (save.store)(found.agent.clone(), save_as.clone(), injected.names.clone(), kept).await.map(|record| (record, bytes))
    };
    let said = String::from_utf8_lossy(&said);
    let said = match &hidden {
        Some(hidden) => injected.redactor.with(std::slice::from_ref(hidden)).redact(&said),
        None => injected.redactor.redact(&said),
    };
    let said = utf16_tail(&said, MAX_OUTPUT);
    let guarded = guard.said();
    let mut text = match &saved {
        Ok((_, bytes)) => format!(
            "Saved the command's output ({bytes} bytes) as the secret {}, which sets {}; it's granted to you alone. Its output isn't shown. Use it with secrets: [\"{}\"].",
            save_as.name.trim(),
            save_as.variable.trim(),
            save_as.name.trim()
        ),
        Err(why) => why.clone(),
    };
    if !said.trim().is_empty() {
        text.push_str(&format!("\n\nIt said (stderr):\n{said}"));
    }
    if !guarded.is_empty() {
        text.push_str(&format!("\n{guarded}"));
    }
    text.push_str(&format!("\n[{state}, {}]", took(began.elapsed().as_secs_f64())));
    let record = saved.as_ref().ok().map(|(record, _)| record.clone());
    Ok(ToolResult::text(
        text,
        json!({"exitCode": exit_code, "exitSignal": exit_signal, "stopped": stopped, "saved": saved.is_ok(), "secret_saved": record}),
    ))
}

/// The value `save` saves from `output`, as what hides it: its text without
/// the spaces and line endings around it, or the bytes for output that
/// isn't text.
fn saved_variable(save: &SaveAs, output: &[u8]) -> SecretVariable {
    let (value, binary) = match std::str::from_utf8(output) {
        Ok(text) => (text.trim().to_owned(), false),
        Err(_) => {
            use base64::Engine as _;
            (base64::engine::general_purpose::STANDARD.encode(output), true)
        }
    };
    SecretVariable {
        secret: save.name.trim().to_owned(),
        variable: save.variable.trim().to_owned(),
        value: Zeroizing::new(value),
        file: binary,
        public: false,
        binary,
    }
}

/// Starts the command and returns at once. It runs on by itself, writing
/// to its log; `background.finished` hears when it ends.
async fn start_background(cwd: &Path, args: BashArgs, background: &Background, injected: &Injected<'_>) -> Result<ToolResult, Error> {
    tokio::fs::create_dir_all(&background.dir).await?;
    let (id, log, file) = new_log(&background.dir)?;
    let mut files = SecretFiles(Some(log.with_extension("secrets")));
    let environment = background.with(injected.environment(files.0.as_deref().unwrap_or(cwd))?);
    let (child, pid) = spawn_wrapped(cwd, &args.command, &log, file, &environment, injected.filter(), background.agent())?;
    files.keep();
    let task = Task { id, command: args.command, pid, log, started: crate::clock::now_ms(), folder: cwd.to_path_buf() };
    injected.used(&task.command, Some(task.id.clone()), task.started);
    let limit = args.timeout.unwrap_or(BACKGROUND_TIMEOUT_SECS).min(MAX_BACKGROUND_SECS);
    let started = format!(
        "Started {} in the background ({}). Its output goes to {}. You'll get an event when it finishes; until then, read the log to check on it, or stop it with `{}`. It's stopped if it runs past {}.",
        task.id,
        group_text(pid),
        task.log.display(),
        stop_hint(pid),
        took(limit),
    );
    let details = json!({"task": task.id, "pid": pid, "log": task.log});
    (background.started)(&task);
    tokio::spawn(watch(child, task, limit, background.finished.clone(), injected.redactor.clone()));
    Ok(ToolResult::text(started, details))
}

/// Waits for a background task to end, stopping it once it has run `limit`
/// seconds (from now), and tells `finished`; `redactor` hides secrets in
/// the end of its output.
async fn watch(mut child: Child, task: Task, limit: f64, finished: Arc<dyn Fn(Finished) + Send + Sync>, redactor: Arc<Redactor>) {
    let pid = task.pid;
    let deadline = sleep(Duration::from_secs_f64(limit));
    tokio::pin!(deadline);
    let mut timed_out = false;
    let mut measure = tokio::time::interval(task_logs::CHECK_EVERY);
    let status = loop {
        tokio::select! {
            status = child.wait() => break status,
            _ = measure.tick() => capped(&task).await,
            () = &mut deadline, if !timed_out => {
                timed_out = true;
                kill_group(pid, SIGTERM);
                sleep(Duration::from_millis(500)).await;
                kill_group(pid, SIGKILL);
            }
        }
    };
    let otherwise = match status {
        Ok(status) => match exit_signal(&status) {
            Some(signal) => ended_text(&signal),
            None => format!("exit {}", status.code().unwrap_or_default()),
        },
        Err(error) => format!("failed: {error}"),
    };
    let mut done = task.finished(otherwise, &redactor);
    if timed_out {
        done.status = format!("{TIME_LIMIT} ({})", took(limit));
    }
    finished(done);
}

/// What a running command has written so far: all of it while it's short,
/// then only its start and its end.
#[derive(Default)]
struct Captured {
    /// All of it, while it's at most `INLINE_OUTPUT` characters.
    whole: Option<String>,
    head: String,
    tail: String,
    characters: usize,
    bytes: u64,
}

impl Captured {
    fn new() -> Captured {
        Captured { whole: Some(String::new()), ..Captured::default() }
    }

    fn add(&mut self, text: &str) {
        self.characters += utf16_len(text);
        if let Some(whole) = &mut self.whole {
            whole.push_str(text);
            if self.characters > INLINE_OUTPUT {
                self.head = utf16_head(whole, PREVIEW_HEAD).to_owned();
                self.tail = utf16_tail(whole, PREVIEW_TAIL).to_owned();
                self.whole = None;
            }
        } else {
            let joined = format!("{}{text}", self.tail);
            self.tail = utf16_tail(&joined, PREVIEW_TAIL).to_owned();
        }
    }

    /// Its output for the result: whole, or its start and end with where the rest is.
    fn shown(&self, log: &Path) -> String {
        match &self.whole {
            Some(whole) => whole.clone(),
            None => {
                let left_out = self.characters.saturating_sub(utf16_len(&self.head) + utf16_len(&self.tail));
                format!("{}\n[… {left_out} characters left out; the whole output is in {}]\n{}", self.head, log.display(), self.tail)
            }
        }
    }
}

/// Reads what the command wrote to its log since `offset`, for the agent's
/// updates and the result, with secrets hidden.
fn read_more(
    file: &mut std::fs::File,
    decoder: &mut Utf8Decoder,
    redactor: &mut StreamRedactor,
    captured: &mut Captured,
    update: Update<'_>,
) -> std::io::Result<()> {
    use std::io::Read;
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        captured.bytes += read as u64;
        let text = redactor.push(&decoder.write(&buffer[..read]));
        if !text.is_empty() {
            captured.add(&text);
            update(ToolResult::text(text, json!({"stream": "output"})));
        }
        if read < buffer.len() {
            return Ok(());
        }
    }
}

/// Runs a command in the foreground the way background tasks run, writing
/// to a log, so that at its timeout it can move to the background and run on
/// (a factoryd restart included) instead of being stopped.
async fn run_logged(
    cwd: &Path,
    args: BashArgs,
    cancel: &CancellationToken,
    detach: &CancellationToken,
    update: Update<'_>,
    background: &Background,
    injected: &Injected<'_>,
) -> Result<ToolResult, Error> {
    tokio::fs::create_dir_all(&background.dir).await?;
    let began = std::time::Instant::now();
    let started = crate::clock::now_ms();
    let (id, log, file) = new_log(&background.dir)?;
    let mut reader = std::fs::File::open(&log)?;
    let mut files = SecretFiles(Some(log.with_extension("secrets")));
    let environment = background.with(injected.environment(files.0.as_deref().unwrap_or(cwd))?);
    let (mut child, pid) = spawn_wrapped(cwd, &args.command, &log, file, &environment, injected.filter(), background.agent())?;
    let timeout = foreground_timeout(args.timeout);
    let sleeping = checks::starts_with_sleep(&args.command);
    let mut decoder = Utf8Decoder::default();
    let mut redactor = StreamRedactor::new(injected.redactor.clone());
    let mut captured = Captured::new();
    let timer = sleep(Duration::from_secs_f64(timeout));
    tokio::pin!(timer);
    let kill_timer = sleep(Duration::from_secs(0));
    tokio::pin!(kill_timer);
    let mut kill_armed = false;
    let mut stopped: Option<&'static str> = None;
    let mut ticker = tokio::time::interval(POLL);
    let status = loop {
        tokio::select! {
            biased;
            () = &mut timer, if stopped.is_none() => {
                if sleeping {
                    stopped = Some("timed out");
                    kill_group(pid, SIGTERM);
                    kill_timer.as_mut().reset(Instant::now() + Duration::from_millis(500));
                    kill_armed = true;
                } else {
                    read_more(&mut reader, &mut decoder, &mut redactor, &mut captured, update)?;
                    let task = Task { id, command: args.command, pid, log, started, folder: cwd.to_path_buf() };
                    files.keep();
                    injected.used(&task.command, Some(task.id.clone()), started);
                    return Ok(moved(task, child, &captured, Moved::Timeout(timeout), began, background, injected.redactor.clone()));
                }
            }
            () = detach.cancelled(), if stopped.is_none() => {
                if sleeping {
                    stopped = Some("stopped so a new message could reach you");
                    kill_group(pid, SIGTERM);
                    kill_timer.as_mut().reset(Instant::now() + Duration::from_millis(500));
                    kill_armed = true;
                } else {
                    read_more(&mut reader, &mut decoder, &mut redactor, &mut captured, update)?;
                    let task = Task { id, command: args.command, pid, log, started, folder: cwd.to_path_buf() };
                    files.keep();
                    injected.used(&task.command, Some(task.id.clone()), started);
                    return Ok(moved(task, child, &captured, Moved::ForMessage, began, background, injected.redactor.clone()));
                }
            }
            () = cancel.cancelled(), if stopped.is_none() => {
                stopped = Some("aborted");
                kill_group(pid, SIGTERM);
                kill_timer.as_mut().reset(Instant::now() + Duration::from_millis(500));
                kill_armed = true;
            }
            () = &mut kill_timer, if kill_armed => {
                kill_armed = false;
                kill_group(pid, SIGKILL);
            }
            status = child.wait() => break status?,
            _ = ticker.tick() => {
                read_more(&mut reader, &mut decoder, &mut redactor, &mut captured, update)?;
                if captured.bytes > LOG_BYTES && stopped.is_none() {
                    stopped = Some("output limit exceeded");
                    kill_group(pid, SIGTERM);
                    kill_timer.as_mut().reset(Instant::now() + Duration::from_millis(500));
                    kill_armed = true;
                }
            }
        }
    };
    if stopped.is_some() {
        kill_group(pid, SIGKILL);
    }
    read_more(&mut reader, &mut decoder, &mut redactor, &mut captured, update)?;
    let mut rest = redactor.push(&decoder.end());
    rest.push_str(&redactor.finish());
    if !rest.is_empty() {
        captured.add(&rest);
        update(ToolResult::text(rest, json!({"stream": "output"})));
    }
    drop(files);
    injected.used(&args.command, None, started);
    let task = Task { id, command: args.command, pid, log, started, folder: cwd.to_path_buf() };
    let recorded = task.recorded_exit();
    let _ = std::fs::remove_file(task.status_path());
    let (exit_code, exit_signal) = match recorded {
        Some(code) if code > 128 => (None, Some(signal_name(code - 128))),
        Some(code) => (Some(code), None),
        None => (None, exit_signal(&status)),
    };
    let state = match (stopped, &exit_signal, exit_code) {
        (Some(reason), _, _) => reason.to_owned(),
        (None, Some(signal), _) => ended_text(signal),
        (None, None, code) => format!("exit {}", code.unwrap_or_default()),
    };
    // Short output comes back whole and its log goes; longer, the log stays
    // for the agent to read (its name then isn't reused).
    let kept = if captured.whole.is_some() {
        let _ = std::fs::remove_file(&task.log);
        None
    } else {
        Some(task.log.clone())
    };
    let shown = captured.shown(kept.as_deref().unwrap_or(&task.log));
    let text = format!("{shown}\n[{state}, {}]", took(began.elapsed().as_secs_f64()));
    Ok(ToolResult::text(
        text,
        json!({"exitCode": exit_code, "exitSignal": exit_signal, "stopped": stopped, "characters": captured.characters, "outputBytes": captured.bytes, "log": kept}),
    ))
}

/// Why a foreground command moved to the background.
enum Moved {
    /// It ran past its timeout, in seconds.
    Timeout(f64),
    /// A message came for the agent.
    ForMessage,
}

/// The command moved to the background: it runs on as a task, and the agent
/// hears when it ends.
fn moved(
    task: Task,
    child: Child,
    captured: &Captured,
    why: Moved,
    began: std::time::Instant,
    background: &Background,
    redactor: Arc<Redactor>,
) -> ToolResult {
    let (id, pid, log) = (task.id.clone(), task.pid, task.log.clone());
    (background.started)(&task);
    tokio::spawn(watch(child, task, MOVED_LIMIT_SECS, background.finished.clone(), redactor));
    let so_far = match &captured.whole {
        Some(whole) => utf16_tail(whole, PREVIEW_TAIL).to_owned(),
        None => captured.tail.clone(),
    };
    let ran = took(began.elapsed().as_secs_f64());
    let (why, advice) = match why {
        Moved::Timeout(timeout) => (
            format!("Still running after {} (its timeout), so it", took(timeout)),
            "\nA command this slow blocks your work. Next time run it with run_in_background, or better, find out why it's slow and fix that: needless copies, cold rebuilds, scanning huge folders.",
        ),
        Moved::ForMessage => (format!("A new message came for you after {ran}, so this command"), ""),
    };
    let text = format!(
        "{why} moved to the background as {id} ({}). It keeps running, for up to {} more, and you'll get an event when it ends; don't start it again meanwhile. Its output goes to {}; stop it with `{}`.{advice}\n\nOutput so far ends with:\n{so_far}\n[moved to the background, {ran}]",
        group_text(pid),
        took(MOVED_LIMIT_SECS),
        log.display(),
        stop_hint(pid),
    );
    ToolResult::text(text, json!({"movedToBackground": id, "task": id, "pid": pid, "log": log}))
}

/// Runs `script` as background tasks run: in its own process group, so it
/// outlives whoever started it, its output appended to `log` and its exit
/// code written to `log` with the extension `status` (removed first). Returns
/// the shell, to reap, and its process group. `environment`: variables set
/// for it on top of what it inherits. `agent`: whose it is (`super::asking`).
pub fn spawn_detached(cwd: &Path, script: &str, log: &Path, environment: &[(String, String)], agent: &str) -> std::io::Result<(Child, u32)> {
    let file = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
    match std::fs::remove_file(log.with_extension("status")) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let environment: Vec<(String, Zeroizing<String>)> =
        environment.iter().map(|(name, value)| (name.clone(), Zeroizing::new(value.clone()))).collect();
    spawn_wrapped(cwd, script, log, file, &environment, None, agent)
}

/// `environment`: variables set for this command only, on top of what it
/// inherits. `filter`: the program (with its arguments) its output goes
/// through to the log, and the values it hides. `agent`: whose it is: what
/// asks who you are in it is stopped, and said in its log (`super::asking`).
fn spawn_wrapped(
    cwd: &Path,
    script: &str,
    log: &Path,
    file: std::fs::File,
    environment: &[(String, Zeroizing<String>)],
    filter: Option<(&[std::ffi::OsString], Zeroizing<String>)>,
    agent: &str,
) -> std::io::Result<(Child, u32)> {
    // Shares the command's own offset in the log, so what it says lands
    // between the command's output, never over it.
    let told = file.try_clone()?;
    // Claktory's own Git Bash, in a job of its own (crate::windows).
    #[cfg(windows)]
    let (child, pid) = {
        let shell = Shell::get()?;
        let mut extra = environment.to_vec();
        if let Some((_, values)) = &filter {
            extra.push((crate::secrets::FILTER_VALUES.to_owned(), values.clone()));
        }
        let environment = shell.environment(std::env::vars_os(), &extra)?;
        shell.spawn_task(cwd, script, log, file, environment, filter.as_ref().map(|(program, _)| *program))?
    };
    #[cfg(unix)]
    let (child, pid) = {
        let mut command = Command::new("bash");
        let wrapper = if filter.is_some() { FILTERED_TASK_WRAPPER } else { TASK_WRAPPER };
        command
            .args(["--noprofile", "--norc", "-c", wrapper, "factory-task", script])
            .arg(log.with_extension("status"))
            .envs(environment.iter().map(|(name, value)| (name, value.as_str())));
        if let Some((program, values)) = filter {
            command.args(program).env(crate::secrets::FILTER_VALUES, values.as_str());
        }
        command.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::from(file.try_clone()?)).stderr(Stdio::from(file)).process_group(0);
        let child = command.spawn()?;
        let pid = child.id().ok_or_else(|| std::io::Error::other("the shell was spawned without a pid"))?;
        (child, pid)
    };
    Guard::watch(pid, agent, Some(told));
    Ok((child, pid))
}

/// A new, numbered log file for a background task. Its number comes after
/// every log there and every one that went (`task_logs::prune`), so no id
/// that went is used again. Opened to append, so the task writes at its end wherever
/// that is, and its middle can be cut.
fn new_log(dir: &Path) -> std::io::Result<(String, PathBuf, std::fs::File)> {
    let mut number = 1;
    for entry in std::fs::read_dir(dir)? {
        if entry?.path().extension().is_some_and(|extension| extension == "log") {
            number += 1;
        }
    }
    number = number.max(task_logs::pruned(dir) + 1);
    loop {
        let id = format!("task-{number}");
        let path = dir.join(format!("{id}.log"));
        match std::fs::OpenOptions::new().append(true).create_new(true).open(&path) {
            Ok(file) => return Ok((id, path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => number += 1,
            Err(error) => return Err(error),
        }
    }
}

#[cfg(windows)]
fn spawn(cwd: &Path, script: &str, environment: &[(String, Zeroizing<String>)]) -> Result<(Child, u32), Error> {
    let shell = Shell::get()?;
    let environment = shell.environment(std::env::vars_os(), environment)?;
    Ok(shell.spawn_piped(cwd, script, environment)?)
}

#[cfg(unix)]
fn spawn(cwd: &Path, script: &str, environment: &[(String, Zeroizing<String>)]) -> Result<(Child, u32), Error> {
    let mut command = Command::new("bash");
    command
        .args(["--noprofile", "--norc", "-e", "-o", "pipefail", "-c", script])
        .envs(environment.iter().map(|(name, value)| (name, value.as_str())))
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let child = command.spawn()?;
    let pid = child.id().ok_or_else(|| std::io::Error::other("the shell was spawned without a pid"))?;
    Ok((child, pid))
}

/// How a command ended, and the end of its output.
struct Ended {
    tail: String,
    characters: usize,
    output_bytes: usize,
    stopped: Option<&'static str>,
    exit_code: Option<i32>,
    exit_signal: Option<String>,
    /// "exit 0", "signal SIGTERM", "timed out"…
    state: String,
}

/// Streams the command's output to `update` until it ends, is stopped, or
/// (with a timeout) runs out of time.
async fn collect(
    mut child: Child,
    pid: u32,
    timeout: Option<f64>,
    cancel: &CancellationToken,
    update: Update<'_>,
    redactor: &Arc<Redactor>,
) -> Result<Ended, Error> {
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let mut output = Output {
        tail: String::new(),
        characters: 0,
        output_bytes: 0,
        stopped: None,
        update,
        stdout: StreamRedactor::new(redactor.clone()),
        stderr: StreamRedactor::new(redactor.clone()),
    };
    let (mut out_decoder, mut err_decoder) = (Utf8Decoder::default(), Utf8Decoder::default());
    let (mut out_buf, mut err_buf) = (vec![0u8; 65_536], vec![0u8; 65_536]);
    let timer = sleep(Duration::from_secs_f64(timeout.unwrap_or(0.0)));
    tokio::pin!(timer);
    let kill_timer = sleep(Duration::from_secs(0));
    tokio::pin!(kill_timer);
    let mut kill_armed = false;
    let mut exit = None;
    while stdout.is_some() || stderr.is_some() || exit.is_none() {
        tokio::select! {
            // Match Node's stable listener order when both pipes are ready. Stop events
            // stay ahead of pipe reads so continuous output cannot starve them.
            biased;
            () = &mut timer, if timeout.is_some() && output.stopped.is_none() => stop(&mut output, pid, "timed out", &mut kill_timer, &mut kill_armed),
            () = cancel.cancelled(), if output.stopped.is_none() => stop(&mut output, pid, "aborted", &mut kill_timer, &mut kill_armed),
            () = &mut kill_timer, if kill_armed => {
                kill_armed = false;
                kill_group(pid, SIGKILL);
            }
            read = async { match stdout.as_mut() { Some(pipe) => pipe.read(&mut out_buf).await, None => std::future::pending().await } }, if stdout.is_some() => {
                match read {
                    Ok(n) if n > 0 => {
                        if output.chunk(&mut out_decoder, &out_buf[..n], "stdout") {
                            stop(&mut output, pid, "output limit exceeded", &mut kill_timer, &mut kill_armed);
                        }
                    }
                    _ => {
                        let rest = out_decoder.end();
                        output.consume(&rest, "stdout", true);
                        stdout = None;
                    }
                }
            }
            read = async { match stderr.as_mut() { Some(pipe) => pipe.read(&mut err_buf).await, None => std::future::pending().await } }, if stderr.is_some() => {
                match read {
                    Ok(n) if n > 0 => {
                        if output.chunk(&mut err_decoder, &err_buf[..n], "stderr") {
                            stop(&mut output, pid, "output limit exceeded", &mut kill_timer, &mut kill_armed);
                        }
                    }
                    _ => {
                        let rest = err_decoder.end();
                        output.consume(&rest, "stderr", true);
                        stderr = None;
                    }
                }
            }
            status = child.wait(), if exit.is_none() => {
                exit = Some(status?);
            }
        }
    }
    if output.stopped.is_some() {
        kill_group(pid, SIGKILL);
    }
    let status = exit.unwrap_or_default();
    let exit_signal = exit_signal(&status);
    // On Windows a process that ended other than by exiting has a code too.
    let exit_code = if exit_signal.is_some() { None } else { status.code() };
    let state = match (output.stopped, &exit_signal, exit_code) {
        (Some(reason), _, _) => reason.to_owned(),
        (None, Some(signal), _) => ended_text(signal),
        (None, None, code) => format!("exit {}", code.unwrap_or_default()),
    };
    Ok(Ended {
        tail: output.tail,
        characters: output.characters,
        output_bytes: output.output_bytes,
        stopped: output.stopped,
        exit_code,
        exit_signal,
        state,
    })
}
