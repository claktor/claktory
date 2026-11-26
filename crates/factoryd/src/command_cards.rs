//! Execute on command cards (Fugo 35295 and 35326). An agent asks the
//! person to run a command on this computer with ask_person's `command`
//! instead of a fenced block they'd copy into a terminal. The card is an
//! action card: Claktory's heading, the command byte for byte, the
//! computer and folder it runs in, labelled warnings (hints, not a gate),
//! and the agent's words below as its reason. Only the person's Approve
//! (AnswerAsk, `confirmed`: an app that showed the command, with the
//! confirm's second tap when there are warnings) runs it, once, as
//! them: their login shell with -c, in a session of its own with no
//! terminal (a password or host-key prompt fails rather than waits), stdin
//! empty, the agents' environment without Claktory's own variables or any
//! secret, and stopped with everything it started at its timeout. What it
//! prints (secrets hidden) shows on the card and goes to the agent; it's
//! kept in `ask_runs`, which db_query can't read, and goes with its card.
//! A run Claktory stopped during (a restart, an update) is stopped too, and
//! never run again.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::factory::{Factory, host_event};
use crate::transcript::now_ms;

/// The longest command a card takes, in characters.
pub(crate) const MAX_COMMAND: usize = 4000;
/// How long a command may run unless the agent asks otherwise, in seconds.
pub(crate) const DEFAULT_TIMEOUT: u32 = 120;
/// The longest it may run, in seconds.
pub(crate) const MAX_TIMEOUT: u32 = 600;
/// What's read of its output; the rest is counted, not kept.
const KEPT: usize = 256 * 1024;
/// What the card shows of it: the end.
const SHOWN: usize = 64 * 1024;
/// What the agent is told of it, in characters: the end, as bash's.
const TOLD: usize = 30_000;
/// Between asking the group to stop and making it.
const GRACE: Duration = Duration::from_secs(5);
/// How often the card follows a run.
const SHOW_EVERY: Duration = Duration::from_secs(1);
/// After the shell ends, how long what it started may still write to its
/// output before it's read no more.
const DRAIN: Duration = Duration::from_millis(500);

/// What a run left. Its output stays until its card does.
pub(crate) const SCHEMA: &str = "
-- Execute cards' runs (command_cards): how each went, and what it printed
-- (the last 64 KB, secrets hidden). pgid and leader_start find a run's
-- process group after a restart, so it's stopped and never a stranger's.
CREATE TABLE ask_runs (
    ask INTEGER PRIMARY KEY,
    state TEXT NOT NULL,
    exit_code INTEGER NOT NULL DEFAULT 0,
    output TEXT NOT NULL DEFAULT '',
    truncated INTEGER NOT NULL DEFAULT 0,
    started INTEGER NOT NULL,
    duration INTEGER NOT NULL DEFAULT 0,
    device TEXT NOT NULL DEFAULT '',
    error TEXT NOT NULL DEFAULT '',
    pgid INTEGER,
    leader_start TEXT
);
";

/// What an Execute card runs, as asked: kept in `asks.action`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Command {
    pub command: String,
    pub cwd: String,
    pub machine: String,
    pub timeout_seconds: u32,
    #[serde(default)]
    pub warnings: Vec<Warning>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Warning {
    pub kind: Kind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    Admin,
    Remote,
    Destructive,
    Services,
    PipeToShell,
    Sensitive,
}

impl Kind {
    fn to_proto(self) -> v1::CommandWarningKind {
        match self {
            Kind::Admin => v1::CommandWarningKind::Admin,
            Kind::Remote => v1::CommandWarningKind::Remote,
            Kind::Destructive => v1::CommandWarningKind::Destructive,
            Kind::Services => v1::CommandWarningKind::Services,
            Kind::PipeToShell => v1::CommandWarningKind::PipeToShell,
            Kind::Sensitive => v1::CommandWarningKind::Sensitive,
        }
    }
}

/// How a run is going, or went.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Run {
    pub state: State,
    pub exit_code: i32,
    pub output: String,
    pub truncated: bool,
    pub started: i64,
    pub duration: i64,
    pub device: String,
    pub error: String,
    /// While it runs: what it waits for, in Claktory's words; '' otherwise.
    pub waiting: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum State {
    #[default]
    Running,
    Exited,
    TimedOut,
    FailedToStart,
    Interrupted,
}

impl State {
    fn name(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Exited => "exited",
            State::TimedOut => "timed_out",
            State::FailedToStart => "failed_to_start",
            State::Interrupted => "interrupted",
        }
    }

    fn from_name(name: &str) -> State {
        match name {
            "exited" => State::Exited,
            "timed_out" => State::TimedOut,
            "failed_to_start" => State::FailedToStart,
            "interrupted" => State::Interrupted,
            _ => State::Running,
        }
    }

    fn to_proto(self) -> v1::CommandRunState {
        match self {
            State::Running => v1::CommandRunState::Running,
            State::Exited => v1::CommandRunState::Exited,
            State::TimedOut => v1::CommandRunState::TimedOut,
            State::FailedToStart => v1::CommandRunState::FailedToStart,
            State::Interrupted => v1::CommandRunState::Interrupted,
        }
    }
}

/// Runs going now, by card (`Factory::command_runs`).
#[derive(Default)]
pub(crate) struct Runs(Mutex<HashMap<i64, Live>>);

/// A run going now.
struct Live {
    /// Its process group; 0 until it started.
    pgid: i32,
    /// How it's going.
    run: Run,
    /// Claktory stopped it as it stopped itself (`stop_all`): the next
    /// start records it and tells its agent, not this one.
    stopping: bool,
}

impl Runs {
    fn with<T>(&self, with: impl FnOnce(&mut HashMap<i64, Live>) -> T) -> T {
        with(&mut crate::locking::lock(&self.0))
    }
}

pub(crate) fn to_proto(command: &Command, run: Option<&Run>) -> v1::AskCommand {
    v1::AskCommand {
        command: command.command.clone(),
        cwd: command.cwd.clone(),
        machine: command.machine.clone(),
        timeout_seconds: command.timeout_seconds,
        warnings: command
            .warnings
            .iter()
            .map(|warning| v1::CommandWarning { kind: warning.kind.to_proto().into(), text: warning.text.clone() })
            .collect(),
        run: run.map(|run| v1::CommandRun {
            state: run.state.to_proto().into(),
            exit_code: run.exit_code,
            output: run.output.clone(),
            output_truncated: run.truncated,
            started_ms: run.started,
            duration_ms: run.duration,
            device: run.device.clone(),
            error: run.error.clone(),
            waiting: run.waiting.clone(),
        }),
    }
}

/// Why `command` can't be on a card, in a sentence; None when it can.
pub(crate) fn refused(command: &str) -> Option<String> {
    if command.trim().is_empty() {
        return Some("command: give the command to run.".to_owned());
    }
    if command.chars().count() > MAX_COMMAND {
        return Some(format!("command: at most {MAX_COMMAND} characters; put a longer one in a script and run that."));
    }
    hiding_in("command", command)
}

/// Whether `c` can make what a card shows differ from what runs: a control
/// character (but a line break or a tab), a line or paragraph separator, or
/// one that shows nothing or reorders what's around it (`text::hidden`:
/// bidi controls, zero widths, fillers, variation selectors, tag
/// characters), and every other invisible format character (Fugo 36500).
fn hiding(c: char) -> bool {
    match c {
        '\n' | '\t' => false,
        c if u32::from(c) < 0x20 => true,
        '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}' => true,
        c if factory_agent::text::hidden(c) => true,
        c => matches!(
            u32::from(c),
            0x0600..=0x0605 | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2 | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        ),
    }
}

/// Why `text`, the card's `what`, can't be shown as it runs: the first
/// character in it that hides or reorders what's shown.
fn hiding_in(what: &str, text: &str) -> Option<String> {
    text.chars().find(|c| hiding(*c)).map(|c| {
        format!("{what}: it holds U+{:04X}, a character that can hide or reorder what's shown; write it with plain characters.", u32::from(c))
    })
}

/// The card's command, checked and with what the person should know of it,
/// or why not. `cwd`: where it runs, absolute; `timeout`: in seconds.
pub(crate) fn prepare(command: &str, cwd: &Path, timeout: Option<u32>) -> Result<Command, String> {
    if let Some(why) = refused(command) {
        return Err(why);
    }
    // Shown as where it runs, so held to the same characters.
    if let Some(why) = hiding_in("cwd", &cwd.to_string_lossy()) {
        return Err(why);
    }
    if !cwd.is_absolute() {
        return Err(format!("cwd: give an absolute folder, not {}.", cwd.display()));
    }
    if !cwd.is_dir() {
        return Err(format!("cwd: {} isn't a folder on this computer.", cwd.display()));
    }
    let timeout_seconds = timeout.unwrap_or(DEFAULT_TIMEOUT);
    let machine = crate::remote::machine_name().to_owned();
    if !(1..=MAX_TIMEOUT).contains(&timeout_seconds) {
        return Err(format!("timeout: 1 to {MAX_TIMEOUT} seconds."));
    }
    Ok(Command {
        command: command.to_owned(),
        cwd: cwd.display().to_string(),
        warnings: warnings(command, &machine, touch_id_for_sudo(Path::new(PAM))),
        machine,
        timeout_seconds,
    })
}

/// Whether the card can still run: its folder is still there.
pub(crate) fn check(command: &Command) -> Result<(), String> {
    if Path::new(&command.cwd).is_dir() {
        Ok(())
    } else {
        Err(format!("Its folder, {}, isn't there any more, so it can't run: dismiss this card.", command.cwd))
    }
}

/// The program a word names: its last path part ("/usr/bin/sudo" → "sudo").
fn program(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// The words of a command, roughly as a shell splits them: quotes dropped,
/// and its operators (`;`, `|`, `&`, parentheses, backquotes) apart.
fn words(command: &str) -> Vec<String> {
    let spaced: String =
        command.chars().map(|c| if matches!(c, ';' | '|' | '&' | '(' | ')' | '`' | '\n' | '{' | '}') { ' ' } else { c }).collect();
    spaced.split_whitespace().map(|word| word.trim_matches(|c| matches!(c, '\'' | '"' | '$')).to_owned()).collect()
}

/// The host `ssh`'s arguments name: the first that isn't an option or one's value.
fn ssh_host(arguments: &[String]) -> Option<String> {
    const WITH_VALUE: &str = "bcDEeFIiJLlmOoPpQRSWw";
    let mut skip = false;
    for word in arguments {
        if skip {
            skip = false;
            continue;
        }
        if let Some(flags) = word.strip_prefix('-') {
            skip = flags.len() == 1 && WITH_VALUE.contains(flags);
            continue;
        }
        return host_of(word);
    }
    None
}

/// "user@host" → "host", and nothing for what can't be a host.
fn host_of(word: &str) -> Option<String> {
    let host = word.rsplit_once('@').map_or(word, |(_, host)| host);
    let fine = !host.is_empty() && host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':' | '[' | ']'));
    fine.then(|| host.chars().take(64).collect())
}

/// The host a copy names (`host:path`, `user@host:path`), if any.
fn copy_host(arguments: &[String]) -> Option<String> {
    arguments
        .iter()
        .filter(|word| !word.starts_with('-') && !word.contains("://"))
        .find_map(|word| word.split_once(':').filter(|(host, _)| !host.is_empty() && !host.contains('/')).and_then(|(host, _)| host_of(host)))
}

const SHELLS: [&str; 11] = ["sh", "bash", "zsh", "dash", "ksh", "fish", "python", "python3", "perl", "ruby", "node"];

/// Where PAM's configuration is.
const PAM: &str = "/etc/pam.d";

/// Whether sudo asks for a fingerprint here (Touch ID for sudo: pam_tid in
/// `pam`'s sudo or sudo_local). A card's command can't be kept from the
/// Mac's screen (an unprivileged process gets no audit session of its own),
/// so sudo there puts up a Touch ID dialog and waits.
fn touch_id_for_sudo(pam: &Path) -> bool {
    ["sudo", "sudo_local"].iter().any(|file| {
        std::fs::read_to_string(pam.join(file)).is_ok_and(|text| {
            text.lines().any(|line| {
                let mut fields = line.split_whitespace();
                fields.next() == Some("auth") && fields.any(|field| field.ends_with("pam_tid.so"))
            })
        })
    })
}

/// What the person should read twice before running `command` on
/// `machine`: hints, by kind, in Claktory's words; never a complete list.
/// `touch_id`: sudo there asks for a fingerprint (`touch_id_for_sudo`).
pub(crate) fn warnings(command: &str, machine: &str, touch_id: bool) -> Vec<Warning> {
    let all = words(command);
    let mut found: Vec<Warning> = Vec::new();
    let mut add = |kind: Kind, text: String| {
        if !found.iter().any(|warning| warning.kind == kind && warning.text == text) {
            found.push(Warning { kind, text });
        }
    };
    for (at, word) in all.iter().enumerate() {
        let rest = &all[at + 1..];
        let next = rest.first().map(String::as_str).unwrap_or_default();
        match program(word) {
            "sudo" if touch_id => add(Kind::Admin, format!("Asks for your fingerprint or password on {machine} (sudo).")),
            name @ ("sudo" | "doas") => add(Kind::Admin, format!("Runs as administrator ({name}).")),
            "su" => add(Kind::Admin, "Runs as another user, the administrator unless it names one (su).".to_owned()),
            "ssh" => match ssh_host(rest) {
                Some(host) => add(Kind::Remote, format!("Connects to {host} (ssh).")),
                None => add(Kind::Remote, "Connects to another computer (ssh).".to_owned()),
            },
            name @ ("scp" | "rsync" | "sftp") => match copy_host(rest).or_else(|| (name == "sftp").then(|| ssh_host(rest)).flatten()) {
                Some(host) => add(Kind::Remote, format!("Connects to {host} ({name}).")),
                None if name != "rsync" => add(Kind::Remote, format!("Connects to another computer ({name}).")),
                None => {}
            },
            "rm" if rest
                .iter()
                .take_while(|word| word.starts_with('-'))
                .any(|flag| flag == "--recursive" || (!flag.starts_with("--") && flag.contains(['r', 'R']))) =>
            {
                add(Kind::Destructive, "Deletes folders and everything in them (rm -r).".to_owned());
            }
            "dd" => add(Kind::Destructive, "Writes straight to a file or disk (dd).".to_owned()),
            name if name.starts_with("mkfs") => add(Kind::Destructive, format!("Formats a disk ({name}).")),
            "diskutil" if ["erase", "partition", "zero", "secureerase", "reformat"].iter().any(|verb| next.to_lowercase().starts_with(verb)) => {
                add(Kind::Destructive, format!("Erases or repartitions a disk (diskutil {next})."));
            }
            "git" if next == "push" && rest.iter().any(|word| word == "-f" || word.starts_with("--force")) => {
                add(Kind::Destructive, "Overwrites a branch on another computer (git push --force).".to_owned());
            }
            "git" if next == "reset" && rest.iter().any(|word| word == "--hard") => {
                add(Kind::Destructive, "Throws away uncommitted changes (git reset --hard).".to_owned());
            }
            name @ ("launchctl" | "systemctl" | "crontab" | "schtasks") => {
                add(Kind::Services, format!("Changes what runs on its own ({name})."));
            }
            "security" => add(Kind::Sensitive, "Reads or changes the keychain (security).".to_owned()),
            _ => {}
        }
    }
    // A download run by a shell: `curl … | sh`, `sh <(curl …)`, `bash -c "$(curl …)"`.
    let fetches = |text: &str| words(text).iter().any(|word| matches!(program(word), "curl" | "wget"));
    let piped = command.split('|').collect::<Vec<_>>();
    let into_shell = piped.windows(2).any(|pair| {
        let next = words(pair[1]);
        let program_word = next.iter().find(|word| !matches!(program(word), "sudo" | "doas" | "env") && !word.contains('='));
        fetches(pair[0]) && program_word.is_some_and(|word| SHELLS.contains(&program(word)))
    });
    let substituted = (command.contains("<(") || command.contains("$(")) && fetches(command) && all.iter().any(|word| SHELLS.contains(&program(word)));
    if into_shell || substituted {
        add(Kind::PipeToShell, "Runs what it downloads, unseen (… | sh).".to_owned());
    }
    let lower = command.to_lowercase();
    if [".factory", ".ssh", "library/keychains"].iter().any(|place| lower.contains(place)) {
        add(Kind::Sensitive, "Touches Claktory's data, ssh keys or the keychain.".to_owned());
    }
    found
}

/// The environment a card's command runs with: an allowlist of what
/// factoryd's own holds (Fugo 36006), so anything it gains later stays out
/// unless added here on purpose, and the agents' PATH. None of Claktory's
/// own variables (FACTORY_*, CLAKTORY_*), and no secret. SSH_AUTH_SOCK
/// stays, so ssh works as in a terminal.
pub(crate) fn environment(home: &crate::home::Home) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    environment_of(std::env::vars_os(), home)
}

/// The variables a card's command gets from factoryd's, by name.
const KEPT_VARIABLES: [&str; 8] = ["DISPLAY", "HOME", "LOGNAME", "SHELL", "SSH_AUTH_SOCK", "TMPDIR", "USER", "__CF_USER_TEXT_ENCODING"];

/// `environment`, from Claktory's own `variables`.
fn environment_of(
    variables: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    home: &crate::home::Home,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let mut environment: Vec<(std::ffi::OsString, std::ffi::OsString)> = variables
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            KEPT_VARIABLES.contains(&name.as_ref()) || name.starts_with("XPC_")
        })
        .collect();
    environment.push(("PATH".into(), crate::update::agents_path(home).into()));
    environment
}

/// The person's login shell, as the user database names it; /bin/sh when
/// it names none that's there.
fn login_shell() -> PathBuf {
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: the buffer outlives the entry read from it, and found is
    // either null or points at entry.
    let result = unsafe { libc::getpwuid_r(libc::getuid(), &raw mut entry, buffer.as_mut_ptr().cast(), buffer.len(), &raw mut found) };
    if result == 0 && !found.is_null() && !entry.pw_shell.is_null() {
        // SAFETY: pw_shell points into buffer, a C string.
        let shell = unsafe { std::ffi::CStr::from_ptr(entry.pw_shell) }.to_string_lossy().into_owned();
        if !shell.is_empty() && Path::new(&shell).is_file() {
            return PathBuf::from(shell);
        }
    }
    PathBuf::from("/bin/sh")
}

/// When process `pid` started, as the system says it: so a group found
/// after a restart is known to be the run's, not one whose pid was reused.
pub(crate) fn started_at(pid: i32) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = i32::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
        // SAFETY: info is a proc_bsdinfo, and size says how big it is.
        let got = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
        (got == size).then(|| format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // Field 22, the start time in clock ticks: the 20th after the name.
        stat.rsplit_once(')')?.1.split_whitespace().nth(19).map(str::to_owned)
    }
}

fn signal_group(pgid: i32, signal: i32) {
    if pgid > 1 {
        // SAFETY: a negative pid signals that process group only.
        unsafe { libc::kill(-pgid, signal) };
    }
}

fn group_alive(pgid: i32) -> bool {
    pgid > 1 && factory_agent::tools::bash::group_alive(pgid.unsigned_abs())
}

/// Programs that ask who you are before running as someone else.
const ASKING: [&str; 3] = ["sudo", "doas", "su"];

/// One of `ASKING` running in process group `pgid`, by name, if any: the
/// run waits for a fingerprint or a password. Once it runs as root its name
/// can't be read, but the file it runs can.
pub(crate) fn asking_in(pgid: i32) -> Option<&'static str> {
    factory_agent::tools::asking::processes(pgid).iter().find_map(|process| {
        let file = process.path.rsplit('/').next().unwrap_or_default();
        ASKING.iter().find(|asking| process.name == **asking || file == **asking).copied()
    })
}

/// How the run of card `ask` is going or went: while it runs, as it is now.
pub(crate) fn run_of(runs: &Runs, db: &rusqlite::Connection, ask: i64) -> Option<Run> {
    if let Some(run) = runs.with(|live| live.get(&ask).map(|live| live.run.clone())) {
        return Some(run);
    }
    db.query_row(
        "SELECT state, exit_code, output, truncated, started, duration, device, error FROM ask_runs WHERE ask = ?1",
        [ask],
        |row| {
            Ok(Run {
                state: State::from_name(&row.get::<_, String>(0)?),
                exit_code: row.get(1)?,
                output: row.get(2)?,
                truncated: row.get(3)?,
                started: row.get(4)?,
                duration: row.get(5)?,
                device: row.get(6)?,
                error: row.get(7)?,
                waiting: String::new(),
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// Cards whose runs went with them.
pub(crate) fn forget(db: &rusqlite::Connection) -> rusqlite::Result<()> {
    db.execute("DELETE FROM ask_runs WHERE ask NOT IN (SELECT id FROM asks)", [])?;
    Ok(())
}

fn save(factory: &Factory, ask: i64, run: &Run) {
    let saved = factory.store.db().execute(
        "INSERT INTO ask_runs (ask, state, exit_code, output, truncated, started, duration, device, error)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (ask) DO UPDATE SET state = ?2, exit_code = ?3, output = ?4, truncated = ?5, duration = ?7, error = ?9",
        params![ask, run.state.name(), run.exit_code, run.output, run.truncated, run.started, run.duration, run.device, run.error],
    );
    if let Err(error) = saved {
        tracing::error!(ask, %error, "could not keep how a card's command went");
    }
}

/// What a run printed: the end of what was read, from a character's start,
/// with the vault's values hidden.
fn shown(factory: &Factory, kept: &[u8], total: usize) -> (String, bool) {
    let mut start = kept.len().saturating_sub(SHOWN);
    while start < kept.len() && (kept[start] & 0xC0) == 0x80 {
        start += 1;
    }
    let text = String::from_utf8_lossy(&kept[start..]);
    (factory.redaction.redact(&text), total > kept.len() - start)
}

/// The person approved card `ask` of `agent`'s on `device`: its command
/// starts now, in the background; the card follows it, and the agent is
/// told how it went.
pub(crate) fn start(factory: &Arc<Factory>, agent: &str, ask: i64, command: Command, device: &str) {
    let run = Run { state: State::Running, started: now_ms(), device: device.to_owned(), ..Run::default() };
    save(factory, ask, &run);
    factory.command_runs.with(|live| live.insert(ask, Live { pgid: 0, run: run.clone(), stopping: false }));
    crate::extensions::ask::show(factory, agent, ask, true);
    tracing::info!(%agent, ask, cwd = %command.cwd, device, "running a card's command");
    let factory = factory.clone();
    let agent = agent.to_owned();
    tokio::spawn(async move {
        let run = execute(&factory, &agent, ask, &command, run).await;
        let stopping = factory.command_runs.with(|live| live.remove(&ask)).is_some_and(|live| live.stopping);
        if stopping {
            // Claktory is stopping: its next start says how it went.
            return;
        }
        save(&factory, ask, &run);
        let _ = factory.store.db().execute("UPDATE ask_runs SET pgid = NULL WHERE ask = ?1", [ask]);
        tracing::info!(%agent, ask, state = run.state.name(), exit_code = run.exit_code, duration = run.duration, "a card's command ended");
        crate::extensions::ask::show(&factory, &agent, ask, true);
        tell(&factory, &agent, &command, &run);
    });
}

/// Runs it, following it on the card, until it ends or its time is up.
async fn execute(factory: &Arc<Factory>, agent: &str, ask: i64, command: &Command, mut run: Run) -> Run {
    let failed = |mut run: Run, why: String| {
        run.state = State::FailedToStart;
        run.error = why;
        run
    };
    let cwd = Path::new(&command.cwd);
    if !cwd.is_dir() {
        return failed(run, format!("Its folder, {}, isn't there any more.", command.cwd));
    }
    let (reader, writer) = match std::io::pipe() {
        Ok(pipe) => pipe,
        Err(error) => return failed(run, format!("Claktory couldn't start it: {error}.")),
    };
    let errors = match writer.try_clone() {
        Ok(errors) => errors,
        Err(error) => return failed(run, format!("Claktory couldn't start it: {error}.")),
    };
    let mut shell = std::process::Command::new(login_shell());
    shell
        .arg("-c")
        .arg(&command.command)
        .current_dir(cwd)
        .env_clear()
        .envs(environment(&factory.home))
        .stdin(std::process::Stdio::null())
        .stdout(writer)
        .stderr(errors);
    // SAFETY: setsid only, between fork and exec: a session and a process
    // group of its own, with no controlling terminal.
    unsafe {
        shell.pre_exec(|| if libc::setsid() == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    let mut shell = tokio::process::Command::from(shell);
    let spawned = shell.spawn();
    // Its copies of the pipe's writing end go, so the reading end ends
    // when the command's do.
    drop(shell);
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return failed(run, format!("It couldn't start: {error}.")),
    };
    let pgid = child.id().and_then(|pid| i32::try_from(pid).ok()).unwrap_or(0);
    let leader = started_at(pgid);
    if let Err(error) = factory.store.db().execute("UPDATE ask_runs SET pgid = ?2, leader_start = ?3 WHERE ask = ?1", params![ask, pgid, leader]) {
        tracing::error!(ask, %error, "could not keep a card's command's process group");
    }
    factory.command_runs.with(|live| {
        if let Some(live) = live.get_mut(&ask) {
            live.pgid = pgid;
        }
    });
    let mut output = match tokio::net::unix::pipe::Receiver::from_owned_fd(OwnedFd::from(reader)) {
        Ok(output) => Some(output),
        Err(error) => {
            tracing::warn!(ask, %error, "could not read a card's command's output");
            None
        }
    };
    let started = tokio::time::Instant::now();
    let deadline = started + Duration::from_secs(u64::from(command.timeout_seconds));
    let mut kept: Vec<u8> = Vec::new();
    let mut total = 0_usize;
    let mut buffer = vec![0_u8; 16 * 1024];
    let mut status = None;
    let mut timed_out = false;
    let mut kill_at: Option<tokio::time::Instant> = None;
    let mut drain_until: Option<tokio::time::Instant> = None;
    let mut ticks = tokio::time::interval(SHOW_EVERY);
    ticks.tick().await;
    loop {
        if status.is_some() && (output.is_none() || drain_until.is_some_and(|until| tokio::time::Instant::now() >= until)) {
            break;
        }
        tokio::select! {
            read = async { output.as_mut().expect("output").readable().await }, if output.is_some() => {
                if read.is_err() {
                    output = None;
                    continue;
                }
                match output.as_ref().expect("output").try_read(&mut buffer) {
                    Ok(0) => output = None,
                    Ok(read) => {
                        total += read;
                        kept.extend_from_slice(&buffer[..read]);
                        if kept.len() > KEPT {
                            kept.drain(..kept.len() - KEPT);
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => output = None,
                }
            }
            exited = child.wait(), if status.is_none() => {
                status = Some(exited);
                drain_until = Some(tokio::time::Instant::now() + DRAIN);
            }
            () = tokio::time::sleep_until(deadline), if !timed_out && status.is_none() => {
                timed_out = true;
                signal_group(pgid, libc::SIGTERM);
                kill_at = Some(tokio::time::Instant::now() + GRACE);
            }
            () = tokio::time::sleep_until(kill_at.unwrap_or(deadline)), if kill_at.is_some() => {
                kill_at = None;
                signal_group(pgid, libc::SIGKILL);
            }
            () = tokio::time::sleep_until(drain_until.unwrap_or(deadline)), if drain_until.is_some() => {}
            _ = ticks.tick() => {
                let (text, truncated) = shown(factory, &kept, total);
                let duration = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
                let waiting = asking_in(pgid).map_or_else(String::new, |name| {
                    format!("Waiting for a fingerprint or password on {} ({name}).", command.machine)
                });
                factory.command_runs.with(|live| {
                    if let Some(live) = live.get_mut(&ask) {
                        live.run.output = text;
                        live.run.truncated = truncated;
                        live.run.duration = duration;
                        live.run.waiting = waiting;
                    }
                });
                crate::extensions::ask::show(factory, agent, ask, true);
            }
        }
    }
    // What it left behind goes too: at once when it timed out; asked
    // first, then made, when the shell ended but something it started runs
    // on (`sleep 3000 &`, `nohup server &`; Fugo 36500).
    if timed_out {
        signal_group(pgid, libc::SIGKILL);
    } else if group_alive(pgid) {
        signal_group(pgid, libc::SIGTERM);
        let until = tokio::time::Instant::now() + GRACE;
        while group_alive(pgid) && tokio::time::Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        signal_group(pgid, libc::SIGKILL);
    }
    let (text, truncated) = shown(factory, &kept, total);
    run.output = text;
    run.truncated = truncated;
    run.duration = i64::try_from(started.elapsed().as_millis()).unwrap_or(i64::MAX);
    match status {
        _ if timed_out => run.state = State::TimedOut,
        Some(Ok(status)) => {
            use std::os::unix::process::ExitStatusExt;
            run.state = State::Exited;
            run.exit_code = status.code().or_else(|| status.signal().map(|signal| 128 + signal)).unwrap_or(-1);
        }
        Some(Err(error)) => {
            run.state = State::Exited;
            run.exit_code = -1;
            run.error = format!("Claktory lost track of it: {error}.");
        }
        None => run.state = State::Exited,
    }
    run
}

/// A fence for `text` that nothing in it closes: more backticks than its
/// longest run of them.
fn fence(text: &str) -> String {
    let longest = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(longest.max(2) + 1)
}

/// The end of `text`, at most `most` characters.
fn tail(text: &str, most: usize) -> &str {
    let count = text.chars().count();
    if count <= most {
        return text;
    }
    let skip = text.char_indices().nth(count - most).map_or(0, |(at, _)| at);
    &text[skip..]
}

/// What the agent is told of the run, in words and its event's summary.
fn told(person: &str, command: &Command, run: &Run) -> (String, String) {
    let took = factory_agent::tools::bash::took(run.duration as f64 / 1000.0);
    let printed = || {
        if run.output.is_empty() {
            "It printed nothing.".to_owned()
        } else {
            let text = tail(&run.output, TOLD);
            let cut = if run.truncated || text.len() < run.output.len() { " (the end of it)" } else { "" };
            let fence = fence(text);
            format!("What it printed{cut}, secrets hidden:\n\n{fence}\n{text}\n{fence}")
        }
    };
    let place = format!("on {} in {}", command.machine, command.cwd);
    match run.state {
        State::Exited => (
            format!("{person} ran your command: exit code {}", run.exit_code),
            format!("{person} ran your command {place}: exit code {}, in {took}. {}", run.exit_code, printed()),
        ),
        State::TimedOut => (
            format!("{person} ran your command, and it ran out of time"),
            format!(
                "{person} ran your command {place}, and it was still running after {} seconds, so it was stopped, with everything it started. {}",
                command.timeout_seconds,
                printed()
            ),
        ),
        State::FailedToStart => {
            (format!("{person}'s run of your command didn't start"), format!("{person} tapped Execute on your command, but it didn't start: {}", run.error))
        }
        State::Interrupted | State::Running => (
            format!("{person}'s run of your command was cut short"),
            format!("{person} ran your command {place}, but {} {}", run.error, printed()),
        ),
    }
}

fn tell(factory: &Arc<Factory>, agent: &str, command: &Command, run: &Run) {
    let person = factory.lock().config.person_capitalized().to_owned();
    let (summary, text) = told(&person, command, run);
    factory.deliver_event(agent, text, host_event(summary));
}

/// Claktory stops: every run still going is stopped with what it started,
/// never to run again. Each stays recorded as running, with why it stopped
/// and what it printed so far, for the next start to record as cut short
/// and tell its agent (`recover`).
pub(crate) fn stop_all(factory: &Factory) {
    let running: Vec<(i64, i32, Run)> = factory.command_runs.with(|live| {
        live.iter_mut()
            .map(|(ask, live)| {
                live.stopping = true;
                (*ask, live.pgid, live.run.clone())
            })
            .collect()
    });
    if running.is_empty() {
        return;
    }
    for (_, pgid, _) in &running {
        signal_group(*pgid, libc::SIGTERM);
    }
    let until = std::time::Instant::now() + Duration::from_secs(2);
    while running.iter().any(|(_, pgid, _)| group_alive(*pgid)) && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    for (ask, pgid, run) in running {
        signal_group(pgid, libc::SIGKILL);
        let _ = factory.store.db().execute(
            "UPDATE ask_runs SET error = ?2, output = ?3, truncated = ?4, duration = ?5, pgid = NULL, leader_start = NULL WHERE ask = ?1 AND state = 'running'",
            params![ask, STOPPED, run.output, run.truncated, run.duration],
        );
    }
}

/// For tests: every run going is left as it is, as if Claktory had died
/// (no signal; its record stays running), so the next start finds it.
pub fn abandon_runs_for_test(factory: &Factory) {
    factory.command_runs.with(|live| live.values_mut().for_each(|live| live.stopping = true));
}

const STOPPED: &str = "Claktory stopped while it ran; it was stopped.";
const GONE: &str = "Claktory stopped while it ran; it may have finished on its own.";

/// A run a stopped Claktory left: its card, agent, process group, the
/// group's leader's start, and why it stopped if Claktory said.
type Left = (i64, String, Option<i32>, Option<String>, String);

/// At start: runs left going by a Claktory that stopped (a restart, an
/// update) are stopped, when their process group is still the run's (its
/// leader started when the run's did; a pid used again is never touched),
/// and recorded as cut short; their agents are told.
pub(crate) fn recover(factory: &Arc<Factory>) {
    let left: Vec<Left> = crate::store::query(
        &factory.store.db(),
        "SELECT r.ask, a.agent, r.pgid, r.leader_start, r.error FROM ask_runs r JOIN asks a ON a.id = r.ask WHERE r.state = 'running'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )
    .unwrap_or_default();
    for (ask, agent, pgid, leader, error) in left {
        let ours = pgid.filter(|pgid| *pgid > 1).filter(|pgid| leader.is_some() && started_at(*pgid) == leader);
        let why = match ours {
            // Stopped as Claktory stopped (`stop_all`).
            None if error == STOPPED => STOPPED,
            Some(pgid) => {
                signal_group(pgid, libc::SIGTERM);
                std::thread::sleep(Duration::from_millis(200));
                signal_group(pgid, libc::SIGKILL);
                tracing::warn!(%agent, ask, pgid, "stopped a card's command a stopped Claktory left running");
                STOPPED
            }
            None => GONE,
        };
        let _ = factory.store.db().execute(
            "UPDATE ask_runs SET state = 'interrupted', error = ?2, pgid = NULL WHERE ask = ?1",
            params![ask, why],
        );
        let command = crate::extensions::ask::command_of(factory, ask);
        crate::extensions::ask::show(factory, &agent, ask, false);
        // The database's lock goes before telling, which takes it again.
        let run = run_of(&factory.command_runs, &factory.store.db(), ask);
        if let (Some(command), Some(run)) = (command, run) {
            tell(factory, &agent, &command, &run);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn characters_that_hide_what_runs_are_refused() {
        for bad in [
            "a\u{0}b",
            "a\rb",
            "a\u{1b}[2Jb",
            "a\u{7f}",
            "a\u{85}",
            "a\u{2028}b",
            "a\u{202e}b",
            "a\u{2066}b",
            "a\u{200b}b",
            "a\u{feff}b",
            "a\u{061c}",
            // Fugo 36500: tags (ASCII smuggling), invisible operators, the
            // Mongolian vowel separator, a soft hyphen, variation selectors,
            // Hangul fillers and the other format characters.
            "rm\u{E0072}\u{E006D} x",
            "a\u{2061}b",
            "a\u{180E}b",
            "r\u{AD}m",
            "a\u{FE0F}",
            "a\u{E0100}",
            "a\u{115F}b",
            "a\u{1160}b",
            "a\u{3164}b",
            "a\u{FFA0}b",
            "a\u{0600}b",
            "a\u{110BD}b",
            "a\u{1D173}b",
        ] {
            assert!(refused(bad).is_some(), "{bad:?}");
        }
        assert!(refused("ls -la\n\tpwd").is_none());
        assert!(refused("echo 'héllo'").is_none(), "other characters are shown, highlighted");
        assert!(refused(&"x".repeat(MAX_COMMAND)).is_none());
        assert!(refused(&"x".repeat(MAX_COMMAND + 1)).is_some());
        assert!(refused("  ").is_some());
    }

    fn kinds(command: &str) -> Vec<(Kind, String)> {
        warnings(command, "Mac", false).into_iter().map(|warning| (warning.kind, warning.text)).collect()
    }

    #[test]
    fn warnings_name_what_to_read_twice() {
        assert_eq!(kinds("ls -la"), []);
        assert_eq!(kinds("ssh prime2 'factoryd allow \"Mac\"'"), [(Kind::Remote, "Connects to prime2 (ssh).".to_owned())]);
        assert_eq!(kinds("ssh -p 2222 -i key matt@box.local uptime"), [(Kind::Remote, "Connects to box.local (ssh).".to_owned())]);
        assert_eq!(kinds("scp file.txt prime2:/tmp/"), [(Kind::Remote, "Connects to prime2 (scp).".to_owned())]);
        assert_eq!(kinds("rsync -a dir/ backup/"), [], "a local copy");
        assert_eq!(kinds("sudo /usr/bin/true")[0].0, Kind::Admin);
        assert_eq!(kinds("cd x && rm -rf build")[0].0, Kind::Destructive);
        assert_eq!(kinds("rm -f one.txt"), [], "not recursive");
        assert_eq!(kinds("git push --force origin main")[0].0, Kind::Destructive);
        assert_eq!(kinds("git reset --hard HEAD~1")[0].0, Kind::Destructive);
        assert_eq!(kinds("diskutil eraseDisk APFS X disk4")[0].0, Kind::Destructive);
        assert_eq!(kinds("launchctl bootout gui/501/x")[0].0, Kind::Services);
        assert_eq!(kinds("curl -fsSL https://x.example/i.sh | sh")[0].0, Kind::PipeToShell);
        assert_eq!(kinds("bash -c \"$(curl -fsSL https://x.example/i.sh)\"")[0].0, Kind::PipeToShell);
        assert_eq!(kinds("curl -s https://x.example | jq .")[..], [], "into another program");
        assert_eq!(kinds("cat ~/.ssh/id_ed25519.pub")[0].0, Kind::Sensitive);
        assert_eq!(kinds("security find-generic-password -s x")[0].0, Kind::Sensitive);
        let both = kinds("ssh prime2 'sudo systemctl restart claktory'");
        let found: Vec<Kind> = both.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(found, [Kind::Remote, Kind::Admin, Kind::Services]);
    }

    #[test]
    fn sudo_with_touch_id_says_it_asks_for_a_fingerprint() {
        let pam = std::env::temp_dir().join(format!("command-cards-pam-{}", std::process::id()));
        std::fs::create_dir_all(&pam).expect("pam");
        assert!(!touch_id_for_sudo(&pam), "no files");
        std::fs::write(pam.join("sudo"), "# sudo: auth account password session\nauth       include        sudo_local\nauth       required       pam_opendirectory.so\n").expect("sudo");
        std::fs::write(pam.join("sudo_local"), "# auth       sufficient     pam_tid.so\n").expect("sudo_local");
        assert!(!touch_id_for_sudo(&pam), "commented out, as macOS's template has it");
        std::fs::write(pam.join("sudo_local"), "auth       sufficient     pam_tid.so\n").expect("sudo_local");
        assert!(touch_id_for_sudo(&pam));
        std::fs::remove_file(pam.join("sudo_local")).expect("gone");
        std::fs::write(pam.join("sudo"), "auth       sufficient     pam_tid.so\nauth       required       pam_opendirectory.so\n").expect("sudo");
        assert!(touch_id_for_sudo(&pam), "in sudo itself");
        std::fs::remove_dir_all(&pam).expect("clean");

        let asks = warnings("sudo launchctl kickstart -k system/x", "Matt's Mac", true);
        assert_eq!(asks[0], Warning { kind: Kind::Admin, text: "Asks for your fingerprint or password on Matt's Mac (sudo).".to_owned() });
        assert_eq!(warnings("doas true", "Matt's Mac", true)[0].text, "Runs as administrator (doas).");
        assert_eq!(warnings("su -", "Matt's Mac", false)[0].kind, Kind::Admin);
    }

    #[test]
    fn what_asks_who_you_are_is_seen_in_the_group() {
        let dir = std::env::temp_dir().join(format!("command-cards-asking-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        // Never the real one: /bin/sleep, named sudo, signed again ad hoc
        // (macOS kills a copy of a system binary that keeps Apple's).
        let sudo = dir.join("sudo");
        std::fs::copy("/bin/sleep", &sudo).expect("copy");
        if cfg!(target_os = "macos") {
            assert!(std::process::Command::new("codesign").args(["-f", "-s", "-"]).arg(&sudo).output().expect("codesign").status.success());
        }
        let mut child = std::process::Command::new(&sudo);
        child.arg("30").process_group(0);
        let mut child = child.spawn().expect("spawn");
        let pgid = i32::try_from(child.id()).expect("pid");
        std::thread::sleep(Duration::from_millis(300));
        assert!(child.try_wait().expect("wait").is_none(), "still running");
        assert_eq!(asking_in(pgid), Some("sudo"));
        child.kill().expect("kill");
        child.wait().expect("wait");
        assert_eq!(asking_in(pgid), None);
        let mut sleep = std::process::Command::new("/bin/sleep").arg("30").process_group(0).spawn().expect("spawn");
        assert_eq!(asking_in(i32::try_from(sleep.id()).expect("pid")), None, "anything else");
        sleep.kill().expect("kill");
        sleep.wait().expect("wait");
        std::fs::remove_dir_all(&dir).expect("clean");
    }

    #[test]
    fn a_fence_outlasts_the_backticks_inside() {
        assert_eq!(fence("plain"), "```");
        assert_eq!(fence("a ``` b"), "````");
        assert_eq!(fence("`````"), "``````");
        assert_eq!(tail("abcdef", 3), "def");
        assert_eq!(tail("ab", 3), "ab");
    }

    #[test]
    fn the_environment_has_none_of_claktory_s_own() {
        let home = crate::home::Home::new(std::env::temp_dir().join("command-cards-env"));
        let factoryd = [
            ("HOME", "/Users/me"),
            ("PATH", "/usr/bin:/bin"),
            ("SSH_AUTH_SOCK", "/private/tmp/launchd/Listeners"),
            ("FACTORY_VAULT_FD", "3"),
            ("FACTORY_SUPERVISED", "1"),
            ("FACTORY_AGENT_ID", "rivet"),
            ("CLAKTORY_SPEECH_MODEL", "/m.gguf"),
            ("TMPDIR", "/var/folders/x/T/"),
            ("XPC_SERVICE_NAME", "0"),
            ("RUST_LOG", "debug"),
            ("AWS_SECRET_ACCESS_KEY", "from a shell"),
            ("__CF_USER_TEXT_ENCODING", "0x1F5:0x0:0x0"),
        ];
        let environment = environment_of(factoryd.iter().map(|(name, value)| (name.into(), value.into())), &home);
        let names: Vec<String> = environment.iter().map(|(name, _)| name.to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["HOME", "SSH_AUTH_SOCK", "TMPDIR", "XPC_SERVICE_NAME", "__CF_USER_TEXT_ENCODING", "PATH"], "only what's allowed");
        let path = environment.last().expect("PATH").1.to_string_lossy().into_owned();
        assert_eq!(path, crate::update::agents_path(&home), "the agents' PATH");
        assert!(super::environment(&home).iter().all(|(name, _)| {
            let name = name.to_string_lossy();
            !name.starts_with("FACTORY_") && !name.starts_with("CLAKTORY_")
        }));
    }

    #[test]
    fn a_process_s_start_is_known_and_a_stranger_s_differs() {
        let me = i32::try_from(std::process::id()).expect("pid");
        let mine = started_at(me).expect("this process's start");
        assert_eq!(started_at(me), Some(mine.clone()), "the same each time");
        assert_ne!(started_at(1), Some(mine));
    }
}
