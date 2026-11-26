//! Changes an admin agent asks the person to approve (#423, Bruno 28259):
//! Rivet turns Continue automatically on or off, uninstalls an app, or
//! widens a pipe only through a card in its conversation (and Needs you),
//! the way agents_clear clears a conversation. The card keeps the change
//! (`Action`, as JSON in `asks.action`); only the person's Approve makes
//! it, checked again then, and the agent is told how it went. Dismissing or
//! replying makes nothing.
//!
//! Narrowing a pipe (pausing it, lowering its limits, taking agents out or
//! off commands, turning a review on) needs no card: Rivet makes it at once
//! with the same tool, and the pipe says who did.

use std::sync::Arc;

use factory_protocol::v1;
use serde::{Deserialize, Serialize};

use crate::factory::Factory;

/// The tools whose result can be such a card.
pub(crate) const TOOLS: [&str; 8] =
    [AUTO_CONTINUE_TOOL, UNINSTALL_TOOL, PIPE_TOOL, GROUP_TOOL, SHARE_TOOL, SHARES_TOOL, REPORTS_TOOL, READER_TOOL];
pub(crate) const AUTO_CONTINUE_TOOL: &str = "models_auto_continue";
pub(crate) const UNINSTALL_TOOL: &str = "apps_uninstall";
pub(crate) const PIPE_TOOL: &str = "pipes_settings";
pub(crate) const GROUP_TOOL: &str = "pipes_group";
pub(crate) const SHARE_TOOL: &str = "pipes_share";
/// Websites shared over pipes, both ways (#502): its `show` asks on a card.
pub(crate) const SHARES_TOOL: &str = "pipes_shares";
/// A pipe's reports waiting for the person: send or discard them, on a
/// card (#471 G6).
pub(crate) const REPORTS_TOOL: &str = "pipes_reports";
/// Letting an agent stop reading reports, on a card (#471 G6).
pub(crate) const READER_TOOL: &str = "pipes_release_reader";

/// What a card's approval does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Action {
    /// Settings → Providers' Continue automatically.
    AutoContinue { on: bool },
    /// Uninstalls an app, keeping its stored data or not.
    UninstallApp { app: String, keep_data: bool },
    /// Changes a pipe.
    Pipe(Box<PipeChange>),
    /// Moves a pipe into a group that shares websites, which it then gets
    /// too (`pipes::shares`): their names and ids when it was asked.
    GroupPipe {
        pipe: String,
        group: String,
        websites: Vec<String>,
        #[serde(default)]
        shares: Vec<String>,
    },
    /// Shares a website on this computer with one pipe (friend bug #32,
    /// Fugo 33421): the port, the name the other side sees, and what
    /// listened there when it was asked, which must still be what listens.
    /// `again`: a share of the person's that ended on that pipe, shared
    /// there again rather than made anew (#502).
    Share {
        pipe: String,
        port: u16,
        name: String,
        listener: Option<Listener>,
        #[serde(default)]
        again: Option<String>,
    },
    /// Shows again a website the other side shares over a pipe, hidden
    /// here (#502, Fugo 35687): its id here, and the pipe's key when it was
    /// asked (`approval_card::key_of`). Void if it ended, isn't hidden any
    /// more, or the key changed.
    ShowShare { pipe: String, share: String, key: String },
    /// Lets an agent stop reading reports (#471 G6, Fugo 33607): its usual
    /// tools back, its conversation started fresh or kept, as the card
    /// said. Refused while anything still forwards reports to it.
    ReleaseReader { agent: String, fresh: bool },
    /// Sends a pipe's waiting reports to its recipient now, or discards
    /// them (#471 G6): bound to those reports (`waiting`, their ids) and,
    /// to send, that recipient.
    DecideReports { pipe: String, send: bool, recipient: String, waiting: Vec<i64> },
    /// Approves a new pipe (#471 G1): only the person's ApprovePipe from
    /// the card does, with their own choices; approving the card alone
    /// never does (`crate::pipes::approval_card`).
    ApprovePipe(Box<crate::pipes::approval_card::Approval>),
    /// Accepts a change in who a pipe's other side is (#471 G7): only the
    /// person's AcceptPipeIdentity from the card does
    /// (`crate::pipes::trust_card`).
    TrustPipe(crate::pipes::trust_card::Trust),
    /// Runs a command on this computer as the person (Execute, Fugo 35295):
    /// started by `crate::extensions::ask::answer` itself, once, never by a
    /// proposal (`crate::command_cards`).
    Execute(Box<crate::command_cards::Command>),
    /// An errand's handoff (#435, `crate::errands::handoff`): Done gives the
    /// browser back to its agent, Can't do this ends the errand; answered
    /// by `crate::extensions::ask::answer` itself.
    Handoff { handoff: Box<crate::errands::HandoffRow> },
}

/// Approving a pipe's card without its choices.
const APPROVE_ON_THE_CARD: &str = "Approve it with the choices on the card, or in the pipe.";
/// Approving a trust card on an app that can't show it.
const ACCEPT_ON_THE_CARD: &str = "Accept it on the card, or in the pipe.";

/// A program listening on a port of this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Listener {
    pub pid: u32,
    pub command: String,
    /// When it started, in clock ticks since boot (/proc/<pid>/stat's 22nd
    /// field), on Linux (W10, Fugo 36064): another program given the same
    /// pid since isn't taken for it. None elsewhere, and on cards from
    /// before it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<u64>,
}

impl Listener {
    /// The same program as `shown`: its pid and command, and its start
    /// when both know it.
    fn is(&self, shown: &Listener) -> bool {
        self.pid == shown.pid && self.command == shown.command && (shown.started.is_none() || self.started == shown.started)
    }
}

/// What listens on a port of this computer, as far as Claktory can tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Heard {
    /// A program of the person's own (this user's), named.
    Mine(Listener),
    /// Another user's program (root's docker-proxy, say): that user's name.
    /// Which program it is can't be seen, and isn't guessed.
    Others(String),
    /// Nothing of the person's, or it can't tell.
    Unknown,
}

/// Tests: ports whose listener is never named (another user's, say).
pub(crate) static UNNAMED: std::sync::Mutex<std::collections::BTreeSet<u16>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// What listens on `port` of this computer now, when it's a program of the
/// person's own: None when nothing of theirs does, or it can't tell.
pub(crate) fn listener(port: u16) -> Option<Listener> {
    match heard(port) {
        Heard::Mine(listener) => Some(listener),
        _ => None,
    }
}

/// What listens on `port` of this computer now: on Linux as /proc says
/// (W10: NixOS keeps lsof elsewhere, Debian may have none), else as lsof
/// does, which sees only this user's programs.
pub(crate) fn heard(port: u16) -> Heard {
    if crate::locking::lock(&UNNAMED).contains(&port) {
        return Heard::Unknown;
    }
    // SAFETY: getuid can't fail.
    let uid = unsafe { libc::getuid() };
    if cfg!(target_os = "linux")
        && let Some(heard) = from_proc(std::path::Path::new("/proc"), port, uid)
    {
        return heard;
    }
    from_lsof(port, uid).map_or(Heard::Unknown, Heard::Mine)
}

/// lsof: in the system's places first, then on PATH and NixOS's (W10;
/// Fugo 36064: it only names what the card shows, granting nothing).
fn lsof() -> Option<std::path::PathBuf> {
    let fixed = ["/usr/sbin/lsof", "/usr/bin/lsof"].into_iter().map(std::path::PathBuf::from);
    let path = std::env::var_os("PATH").map(|path| std::env::split_paths(&path).map(|dir| dir.join("lsof")).collect::<Vec<_>>()).unwrap_or_default();
    let nix = ["/run/current-system/sw/bin/lsof"].into_iter().map(std::path::PathBuf::from);
    fixed.chain(path).chain(nix).find(|path| path.is_absolute() && path.is_file())
}

/// The program of user `uid` lsof sees listening on `port`: the one a
/// shared website reaches, when several do (`reached_first`).
fn from_lsof(port: u16, uid: u32) -> Option<Listener> {
    let output = std::process::Command::new(lsof()?)
        .args(["-nP", "-a", "+c", "0", "-sTCP:LISTEN", &format!("-iTCP:{port}"), "-u", &uid.to_string(), "-Fpctn"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    from_lsof_fields(&String::from_utf8_lossy(&output.stdout))
}

/// lsof's fields (`-Fpctn`): each process (`p`, `c`), then each socket it
/// listens on, its family (`t`) and address (`n`); the program whose
/// socket a shared website reaches first.
fn from_lsof_fields(text: &str) -> Option<Listener> {
    let mut best: Option<(u8, Listener)> = None;
    let (mut pid, mut command, mut v6) = (None, String::new(), false);
    for line in text.lines() {
        let (field, value) = (line.get(..1).unwrap_or_default(), line.get(1..).unwrap_or_default().trim());
        match field {
            "p" => (pid, command) = (value.parse().ok(), String::new()),
            "c" => command = value.to_owned(),
            "f" => v6 = false,
            "t" => v6 = value == "IPv6",
            "n" => {
                let (Some(pid), Some(rank)) = (pid, value.rsplit_once(':').and_then(|(address, _)| reached_first(v6, address))) else { continue };
                if best.as_ref().is_none_or(|(best, _)| rank < *best) {
                    best = Some((rank, Listener { pid, command: command.clone(), started: None }));
                }
            }
            _ => {}
        }
    }
    best.map(|(_, listener)| listener)
}

/// How soon a connection to a shared website reaches a socket listening at
/// `address` (lsof's spelling: "127.0.0.1", "*", "[::1]"): it tries
/// 127.0.0.1, then [::1] (`shares::connect`), and a socket bound to an
/// address beats a wildcard (Abbacchio row 1339). Lower is sooner; None
/// for one it never reaches (an address of the network's).
fn reached_first(v6: bool, address: &str) -> Option<u8> {
    match (v6, address) {
        (false, "127.0.0.1") => Some(0),
        (false, "*" | "0.0.0.0") => Some(1),
        // A dual-stack wildcard takes 127.0.0.1 too.
        (true, "*" | "[::]") => Some(2),
        (true, "[::1]") => Some(3),
        _ => None,
    }
}

/// What listens on `port`, from a /proc (Linux's, or a test's): its TCP
/// tables give each listening socket's inode and owner. Another user's
/// anywhere on the port makes it theirs; the person's own is named by the
/// process holding that very socket (fd -> socket:[inode]), its command and
/// start. None when there are no tables to read: lsof is asked then.
fn from_proc(proc: &std::path::Path, port: u16, uid: u32) -> Option<Heard> {
    let mut tables = 0;
    // Each listening socket's inode, owner and how soon a shared website
    // reaches it (`reached_first`).
    let mut sockets: Vec<(u64, u32, Option<u8>)> = Vec::new();
    for (table, v6) in [("net/tcp", false), ("net/tcp6", true)] {
        let Ok(text) = std::fs::read_to_string(proc.join(table)) else { continue };
        tables += 1;
        // sl local_address rem_address st tx:rx tr:when retrnsmt uid timeout inode
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 10 || fields[3] != "0A" {
                continue;
            }
            let Some((address, local)) = fields[1].rsplit_once(':') else { continue };
            if u16::from_str_radix(local, 16).ok() != Some(port) {
                continue;
            }
            // The tables' hex, in the kernel's byte order.
            let address = match (v6, address) {
                (false, "0100007F") => "127.0.0.1",
                (false, "00000000") => "*",
                (true, "00000000000000000000000000000000") => "*",
                (true, "00000000000000000000000001000000") => "[::1]",
                _ => "",
            };
            if let (Ok(owner), Ok(inode)) = (fields[7].parse(), fields[9].parse()) {
                sockets.push((inode, owner, reached_first(v6, address)));
            }
        }
    }
    if tables == 0 {
        return None;
    }
    if let Some((_, owner, _)) = sockets.iter().find(|(_, owner, _)| *owner != uid) {
        return Some(Heard::Others(user_name(*owner)));
    }
    // The person's sockets a shared website reaches, by how soon.
    let reached: Vec<(String, u8)> =
        sockets.iter().filter_map(|(inode, _, rank)| rank.map(|rank| (format!("socket:[{inode}]"), rank))).collect();
    if reached.is_empty() {
        return Some(Heard::Unknown);
    }
    let processes = std::fs::read_dir(proc).ok()?;
    let mut best: Option<(u8, Listener)> = None;
    for process in processes.flatten() {
        let Some(pid) = process.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else { continue };
        let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else { continue };
        let held = fds
            .flatten()
            .filter_map(|fd| std::fs::read_link(fd.path()).ok())
            .filter_map(|target| reached.iter().find(|(inode, _)| target.as_os_str() == inode.as_str()).map(|(_, rank)| *rank))
            .min();
        let Some(rank) = held else { continue };
        if best.as_ref().is_some_and(|(best, _)| *best <= rank) {
            continue;
        }
        let command = std::fs::read_to_string(process.path().join("comm")).unwrap_or_default().trim().to_owned();
        // Its 22nd field, counted after the command, which may hold spaces
        // and parentheses.
        let stat = std::fs::read_to_string(process.path().join("stat")).unwrap_or_default();
        let started = stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().nth(19)).and_then(|ticks| ticks.parse().ok());
        best = Some((rank, Listener { pid, command, started }));
    }
    Some(best.map_or(Heard::Unknown, |(_, listener)| Heard::Mine(listener)))
}

/// A user's name, or their number when the system has none for them.
fn user_name(uid: u32) -> String {
    // SAFETY: getpwuid_r fills `entry` from `buffer`, both ours and big
    // enough; its name is copied before either goes away.
    unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        let mut buffer = vec![0 as libc::c_char; 16 * 1024];
        if libc::getpwuid_r(uid, &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found) == 0
            && !found.is_null()
            && !entry.pw_name.is_null()
            && let Ok(name) = std::ffi::CStr::from_ptr(entry.pw_name).to_str()
        {
            // Shown as a share's name is (Fugo 36151): no control, bidi or
            // invisible character reaches Rivet's text or a card.
            let name = crate::pipes::shares::shown(name);
            if !name.is_empty() {
                return name;
            }
        }
    }
    format!("user {uid}")
}

/// What listens on `port` when it's another user's program (W10, Fugo
/// 36064): whose, never a guess at which; a card can't share it.
pub(crate) fn others_program(user: &str, port: u16) -> String {
    format!("A program run by {user} listens on port {port}, and Claktory can't see which")
}

/// Whether something answers on `port` of this computer, at 127.0.0.1 or
/// [::1] only, as a shared website is reached.
pub(crate) fn answers(port: u16) -> bool {
    let wait = std::time::Duration::from_millis(500);
    [std::net::Ipv4Addr::LOCALHOST.into(), std::net::Ipv6Addr::LOCALHOST.into()]
        .into_iter()
        .any(|ip: std::net::IpAddr| std::net::TcpStream::connect_timeout(&std::net::SocketAddr::new(ip, port), wait).is_ok())
}

/// Why an admin agent's card can't share a website with `record`, in a
/// sentence; None when it can: open, both people approved it, its other
/// side's key pinned, and no other account than the person accepted (Fugo
/// 33421). Checked when it's asked and again at Approve.
pub(crate) fn share_refusal(factory: &Factory, record: &crate::pipes::Record) -> Option<String> {
    if record.state != crate::pipes::State::Open || !record.approved || !record.peer_approved {
        return Some("Only an open pipe that both people approved can get a website.".to_owned());
    }
    if crate::pipes::identity::changed_account(factory, &record.id) {
        return Some("The pipe's Claktory now belongs to a different account, until the person accepts it.".to_owned());
    }
    if crate::pipe_link::pinned(factory, &record.id).is_none() {
        return Some("The pipe's other side isn't pinned to a key yet, so it can't get a website.".to_owned());
    }
    None
}

/// Why an agent can't be let stop reading reports now, in a sentence; None
/// when it can (#471 G6: refused while anything still forwards to it).
pub(crate) fn release_refusal(factory: &Factory, agent: &str) -> Option<String> {
    let Some(name) = factory.lock().config.agent(agent).map(|found| found.name.clone()) else {
        return Some("That agent doesn't exist any more.".to_owned());
    };
    if !crate::pipes::handoff::reader(factory, agent) {
        return Some(format!("{name} doesn't read reports."));
    }
    match crate::pipes::handoff::forwarding_to(factory, agent) {
        None => Some(format!("Claktory couldn't check where reports go, so {name} still reads them. Try again.")),
        Some(places) if !places.is_empty() => Some(format!(
            "Reports still go to {name} from {} {}: choose who reads them there first.",
            places.len(),
            if places.len() == 1 { "place" } else { "places" }
        )),
        Some(_) => None,
    }
}

/// Who reads a pipe's reports now: its forward's recipient, while it reads
/// reports.
pub(crate) fn reports_recipient(factory: &Factory, pipe: &str) -> Option<String> {
    crate::pipes::handoff::forward(factory, crate::pipes::handoff::Target::Pipe(pipe))
        .map(|forward| forward.recipient)
        .filter(|recipient| !recipient.is_empty() && crate::pipes::handoff::reader(factory, recipient))
}

/// Whether the website the card showed is still the one to share: the
/// same program answering on the port, which is still one that may be
/// shared (friend bug #32: else the card is void).
pub(crate) fn same_website(factory: &Factory, port: u16, shown: &Option<Listener>) -> Result<(), String> {
    // A program Claktory can't name (another user's, a system service's,
    // or no lsof here) is never shared from a card (Fugo 33782).
    let problem = crate::pipes::shares::port_problem(factory, u32::from(port)).or_else(|| crate::pipes::shares::backend_problem(port));
    if let Some(why) = problem {
        return Err(format!("{why} Dismiss this card."));
    }
    if !answers(port) {
        return Err(format!("Nothing answers on port {port} now, so there's nothing to share: dismiss this card."));
    }
    let same = match (heard(port), shown) {
        (Heard::Mine(now), Some(shown)) => now.is(shown),
        (Heard::Others(user), _) => {
            return Err(format!(
                "{}, so it can't be shared from this card. You can share it yourself from the pipe's settings; dismiss this card.",
                others_program(&user, port)
            ));
        }
        _ => false,
    };
    if !same {
        return Err(format!("A different program listens on port {port} now: dismiss this card and ask again."));
    }
    Ok(())
}

/// A change to a pipe, as pipes_settings asks for it: agents by id.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PipeChange {
    pub pipe: String,
    pub pause: bool,
    pub resume: bool,
    pub close: bool,
    /// The whole new limits, when they change.
    pub limits: Option<PipeLimits>,
    /// Agents that become the pipe's, and those that stop being.
    pub add: Vec<String>,
    pub remove: Vec<String>,
    /// Agents that can no longer run commands on this computer, and those
    /// that can. One added can't unless it's in `commands`.
    pub quarantine: Vec<String>,
    pub commands: Vec<String>,
    pub lead: Option<String>,
    /// A new liaison agent, by the name it gets ("" for the one Claktory
    /// suggests).
    pub liaison: Option<String>,
    pub review_outgoing: Option<bool>,
    pub review_incoming: Option<bool>,
    /// The agents' names when it was asked, by id: a card names an agent
    /// deleted since as the person knew it.
    pub names: std::collections::BTreeMap<String, String>,
    /// Resuming a pipe the loop breaker paused: that pause, by when it
    /// paused (0 for one from before it was kept). It continues only that
    /// one (friend bug #32, Fugo 33421).
    pub looped_at: Option<i64>,
    /// A new name, what it's for and instructions (#471 G2), and all three
    /// as they were when it was asked: the card is void if they changed.
    pub name: Option<String>,
    pub purpose: Option<String>,
    pub instructions: Option<String>,
    pub words_were: Option<PipeWords>,
    /// Its liaison's daily spending limit, in dollars (#471 G5): the
    /// liaison, and the limit when it was asked.
    pub spend_cap_usd: Option<f64>,
    pub spend_agent: String,
    pub spend_cap_was: f64,
    /// Deletes the pipe, closed already (#471 G8).
    pub delete: bool,
    /// The person's websites it stops sharing with the pipe, by share id:
    /// their names when it was asked (#471 G4). Always a narrowing.
    pub stop_sharing: std::collections::BTreeMap<String, String>,
}

/// A pipe's name, what it's for and its instructions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PipeWords {
    pub name: String,
    pub purpose: String,
    pub instructions: String,
}

impl PipeWords {
    pub(crate) fn of(record: &crate::pipes::Record) -> PipeWords {
        PipeWords { name: record.name.clone(), purpose: record.purpose.clone(), instructions: record.prompt.clone() }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PipeLimits {
    pub messages_per_hour: u32,
    pub burst: u32,
    pub file_mb_per_day: u32,
}

impl PipeChange {
    /// What in it widens the pipe, in words: what needs the person's card.
    /// Empty when it only narrows it.
    pub(crate) fn widens(&self, record: &crate::pipes::Record, name: &dyn Fn(&str) -> String) -> Vec<String> {
        let mut wider = Vec::new();
        if self.resume {
            wider.push("resume it".to_owned());
        }
        if self.close {
            wider.push("close it".to_owned());
        }
        if !self.add.is_empty() {
            wider.push(format!("add {}", names(&self.add, name)));
        }
        if !self.commands.is_empty() {
            wider.push(format!("let {} run commands on this computer", names(&self.commands, name)));
        }
        if let Some(lead) = &self.lead {
            wider.push(format!("make {} its lead", name(lead)));
        }
        if let Some(liaison) = &self.liaison {
            wider.push(if liaison.is_empty() {
                "add a new agent answering it".to_owned()
            } else {
                format!("add a new agent, {liaison}, answering it")
            });
        }
        if let Some(limits) = self.limits
            && (limits.messages_per_hour > record.limits.per_hour
                || limits.burst > record.limits.burst
                || limits.file_mb_per_day > record.limits.file_mb)
        {
            wider.push(format!("raise its limits to {}", limits_words(limits)));
        }
        if self.name.is_some() {
            wider.push("rename it".to_owned());
        }
        if self.purpose.is_some() {
            wider.push("change what it's for".to_owned());
        }
        if self.instructions.is_some() {
            wider.push("change its instructions".to_owned());
        }
        if let Some(usd) = self.spend_cap_usd
            && usd > self.spend_cap_was
        {
            wider.push(format!("raise its agent's daily spending limit to {}", dollars(usd)));
        }
        if self.delete {
            wider.push("delete it".to_owned());
        }
        if self.review_outgoing == Some(false) && record.review {
            wider.push("turn Review outgoing off".to_owned());
        }
        if self.review_incoming == Some(false) && record.review_incoming {
            wider.push("turn Review incoming off".to_owned());
        }
        wider
    }

    /// Whether a resume still finds the pipe paused as it was asked: one
    /// asked for a loop pause continues only that pause, and one asked for
    /// any other never continues the loop breaker's (friend bug #32).
    pub(crate) fn paused_as_asked(&self, record: &crate::pipes::Record) -> Result<(), String> {
        use crate::pipes::{Pause, State};
        if !self.resume {
            return Ok(());
        }
        let looped = (record.state == State::Paused && record.paused == Pause::Loop).then(|| record.looped.map_or(0, |looped| looped.at));
        match (self.looped_at, looped) {
            (Some(asked), Some(now)) if asked == now => Ok(()),
            (None, None) => Ok(()),
            (Some(_), None) => Err("The pipe isn't paused for looping any more, so there's nothing to continue: dismiss this card.".to_owned()),
            (_, Some(_)) => Err(
                "The pipe paused again after this card was asked, so it's for an earlier pause: dismiss it. The new pause needs a card of its own.".to_owned(),
            ),
        }
    }

    /// Everything it does, in words (Abbacchio's LOWs): asked for ("pause
    /// the pipe "Launch" and add Mia to it") or done ("Paused the pipe
    /// "Launch""), naming the pipe the first time (`pipe`, "the pipe
    /// "Launch"" or "this pipe") and "it" after.
    pub(crate) fn describe(&self, tense: Tense, pipe: &str, name: &dyn Fn(&str) -> String) -> Vec<String> {
        let done = tense == Tense::Done;
        let mut said: Vec<String> = Vec::new();
        let mut first = true;
        let mut it = || if std::mem::take(&mut first) { pipe.to_owned() } else { "it".to_owned() };
        let verb = |ask: &str, past: &str| if done { past.to_owned() } else { ask.to_owned() };
        if self.pause {
            said.push(format!("{} {}", verb("pause", "paused"), it()));
        }
        if self.resume {
            said.push(format!("{} {}", verb("resume", "resumed"), it()));
        }
        if !self.add.is_empty() {
            said.push(format!("{} {} to {}", verb("add", "added"), self.names(&self.add, name), it()));
        }
        if !self.remove.is_empty() {
            said.push(format!("{} {} out of {}", verb("take", "took"), self.names(&self.remove, name), it()));
        }
        if !self.commands.is_empty() {
            said.push(format!("let {} run commands on this computer in {}", self.names(&self.commands, name), it()));
        }
        if !self.quarantine.is_empty() {
            said.push(format!(
                "{} {} running commands on this computer in {}",
                verb("stop", "stopped"),
                self.names(&self.quarantine, name),
                it()
            ));
        }
        if let Some(lead) = &self.lead {
            said.push(format!("{} {} the lead of {}", verb("make", "made"), self.names(std::slice::from_ref(lead), name), it()));
        }
        if let Some(liaison) = &self.liaison {
            // The person's words: never "liaison" (Trish 35998).
            let who = if liaison.is_empty() { "a new agent".to_owned() } else { format!("a new agent, {liaison},") };
            said.push(format!("{} {who} answering {}", verb("add", "added"), it()));
        }
        if let Some(limits) = self.limits {
            let whose = match it().as_str() {
                "it" => "its limits".to_owned(),
                named => format!("the limits of {named}"),
            };
            said.push(format!("set {whose} to {}", limits_words(limits)));
        }
        for (review, label) in [(self.review_outgoing, "Review outgoing"), (self.review_incoming, "Review incoming")] {
            if let Some(on) = review {
                let state = if on { "on" } else { "off" };
                said.push(format!("{} {label} {state} for {}", verb("turn", "turned"), it()));
            }
        }
        if let Some(name) = &self.name {
            said.push(format!("{} {} to \"{}\"", verb("rename", "renamed"), it(), crate::pipes::agents::escape(name)));
        }
        if self.purpose.is_some() {
            said.push(format!("{} what {} is for", verb("change", "changed"), it()));
        }
        if self.instructions.is_some() {
            let whose = match it().as_str() {
                "it" => "its instructions".to_owned(),
                named => format!("the instructions of {named}"),
            };
            said.push(format!("{} {whose}", verb("change", "changed")));
        }
        if let Some(usd) = self.spend_cap_usd {
            let whose = match it().as_str() {
                "it" => "its agent's daily spending limit".to_owned(),
                named => format!("the daily spending limit of the agent answering {named}"),
            };
            // From what it was, as the words' cards show (Abbacchio row 1232).
            said.push(format!("set {whose} from {} to {}", dollars(self.spend_cap_was), dollars(usd)));
        }
        if !self.stop_sharing.is_empty() {
            let websites: Vec<String> =
                self.stop_sharing.values().map(|name| format!("\"{}\"", crate::pipes::agents::escape(name))).collect();
            said.push(format!("{} sharing {} with {}", verb("stop", "stopped"), listed(&websites), it()));
        }
        if self.close {
            said.push(format!("{} {}", verb("close", "closed"), it()));
        }
        if self.delete {
            said.push(format!("{} {}", verb("delete", "deleted"), it()));
        }
        said
    }

    /// Whether it does what no proposal's grant covers (#471): new words,
    /// a higher spending limit, or deleting the pipe.
    pub(crate) fn only_on_a_card(&self) -> bool {
        self.name.is_some()
            || self.purpose.is_some()
            || self.instructions.is_some()
            || self.spend_cap_usd.is_some_and(|usd| usd > self.spend_cap_was)
            || self.delete
    }

    /// The card's lines for new words (#471 G2): each one's old and new
    /// text, and who sees it.
    pub(crate) fn words_shown(&self) -> String {
        let was = self.words_were.clone().unwrap_or_default();
        let mut lines = Vec::new();
        let quoted = |text: &str| if text.is_empty() { "(none)".to_owned() } else { format!("\"{}\"", crate::pipes::agents::escape(text)) };
        if let Some(name) = &self.name {
            lines.push(format!("Name (you and your agents see it): {} → {}.", quoted(&was.name), quoted(name)));
        }
        if let Some(purpose) = &self.purpose {
            lines.push(format!(
                "What it's for (the other side keeps what it was told when you connected): {} → {}.",
                quoted(&was.purpose),
                quoted(purpose)
            ));
        }
        if let Some(instructions) = &self.instructions {
            lines.push(format!("Instructions (only your agents see them): {} → {}.", quoted(&was.instructions), quoted(instructions)));
        }
        lines.join(" ")
    }

    /// Whether what the card acts on is still as it was asked (#471, Fugo's
    /// rule C): its words, and its liaison's limit.
    pub(crate) fn still_as_asked(&self, factory: &Factory, record: &crate::pipes::Record) -> Result<(), String> {
        if (self.name.is_some() || self.purpose.is_some() || self.instructions.is_some())
            && self.words_were.as_ref() != Some(&PipeWords::of(record))
        {
            return Err("The pipe's name, what it's for or its instructions changed since this card was asked: dismiss it, and ask again."
                .to_owned());
        }
        if self.spend_cap_usd.is_some() {
            let now = crate::pipes::spend::caps(&factory.store.db(), &self.spend_agent).map(|(usd, _)| usd).unwrap_or_default();
            if now != self.spend_cap_was || record.lead != self.spend_agent {
                return Err(
                    "The pipe's liaison or its spending limit changed since this card was asked: dismiss it, and ask again.".to_owned()
                );
            }
        }
        Ok(())
    }

    /// Agents by the names they had when it was asked (an agent deleted
    /// since still reads as it did), else as they are now.
    fn names(&self, ids: &[String], name: &dyn Fn(&str) -> String) -> String {
        names(ids, &|id| self.names.get(id).cloned().unwrap_or_else(|| name(id)))
    }
}

/// How a change is told: asked for, or done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tense {
    Ask,
    Done,
}

/// "Paused the pipe "Launch" and added Mia to it": the first word capital.
pub(crate) fn sentence(parts: &[String]) -> String {
    let said = listed(parts);
    let mut chars = said.chars();
    chars.next().map_or_else(String::new, |first| first.to_uppercase().chain(chars).collect())
}

/// Opens the card asking the person to approve `action` for `asker` (an
/// admin agent, checked by the tool): the tool's result, which shows it.
/// `description` is what approving it does, in Claktory's words; `reason`
/// the agent's, shown apart.
pub(crate) fn ask(
    factory: &Factory,
    asker: &str,
    title: &str,
    description: &str,
    reason: &str,
    action: &Action,
) -> Result<factory_agent::ToolResult, String> {
    ask_saying(factory, asker, (title, description, reason), action, "")
}

/// `ask`, its answer starting with `before`.
fn ask_saying(
    factory: &Factory,
    asker: &str,
    (title, description, reason): (&str, &str, &str),
    action: &Action,
    before: &str,
) -> Result<factory_agent::ToolResult, String> {
    let json = serde_json::to_string(action).map_err(|error| format!("Couldn't ask: {error}"))?;
    let id = crate::extensions::ask::ask_to_approve(factory, asker, title, (description, reason), &json)?;
    let body = format!("{description} {reason}");
    let person = factory.lock().config.person().to_owned();
    let said = format!(
        "{before}Asked {person}: they see a card to approve or dismiss it, and only their approval makes the change. You'll be told which in an event. Its id is {id}: if it stops applying before they answer on the card (they answer it in your chat, say), take it back with ask_withdraw."
    );
    let mut details = serde_json::json!({
        "ask": id,
        "title": title,
        "body": body,
        "wants_text": false,
        "action_description": description,
        "action_reason": reason,
    });
    let routes = crate::pipes::agents::routes(factory, asker);
    if !routes.is_empty() {
        details["taint"] = crate::pipes::agents::routes_json(&routes);
    }
    Ok(factory_agent::ToolResult::text(said, details))
}

/// `ask`, unless `proposal` (an approved proposal's id) covers `action`
/// (`crate::proposal_grants`): then the change is made now, with no card,
/// and History says it was approved in that proposal. Not covered, the card
/// opens as without one, and the answer says why.
pub(crate) async fn ask_or_proposed(
    factory: &Arc<Factory>,
    asker: &str,
    title: &str,
    description: &str,
    reason: &str,
    action: &Action,
    proposal: Option<&str>,
) -> Result<factory_agent::ToolResult, String> {
    let Some(proposal) = proposal.map(str::trim).filter(|proposal| !proposal.is_empty()) else {
        return ask(factory, asker, title, description, reason, action);
    };
    let fits = |grant: &crate::proposal_grants::Grant| crate::proposal_grants::fits_action(factory, grant, action);
    let claimed = match crate::proposal_grants::claim(factory, asker, proposal, fits) {
        Ok(claimed) => claimed,
        Err(why) => {
            let said = format!("Proposal {proposal} doesn't cover it: {why}. ");
            return ask_saying(factory, asker, (title, description, reason), action, &said);
        }
    };
    let (me, person) = {
        let inner = factory.lock();
        (inner.config.agent(asker).map_or_else(|| asker.to_owned(), |me| me.name.clone()), inner.config.person().to_owned())
    };
    let made = match action {
        Action::Pipe(change) => {
            let by = format!("{me}, as {person} approved in {}'s proposal,", claimed.proposer_name);
            apply_pipe(factory, change, Some((asker, &by))).await
        }
        other => match serde_json::to_string(other) {
            Ok(json) => apply(factory, &json).await,
            Err(error) => Err(error.to_string()),
        },
    };
    match made {
        Ok(done) => {
            crate::proposals::refresh(factory, claimed.proposal);
            Ok(factory_agent::ToolResult::text(
                format!("{done} {}", claimed.said(&person)),
                serde_json::json!({"proposal_grant": claimed.reference()}),
            ))
        }
        Err(why) => {
            crate::proposal_grants::unclaim(factory, &claimed);
            Err(format!("{why} Nothing was changed."))
        }
    }
}

/// "$12.50".
pub(crate) fn dollars(usd: f64) -> String {
    if usd.fract() == 0.0 { format!("${usd:.0}") } else { format!("${usd:.2}") }
}

/// "20 messages an hour, 5 in a row and 50 MB of files a day".
fn limits_words(limits: PipeLimits) -> String {
    format!("{} messages an hour, {} in a row and {} MB of files a day", limits.messages_per_hour, limits.burst, limits.file_mb_per_day)
}

/// "Ana", "Ana and Bo", "Ana, Bo and Cy".
fn names(ids: &[String], name: &dyn Fn(&str) -> String) -> String {
    let named: Vec<String> = ids.iter().map(|id| name(id)).collect();
    factory_agent::text::list_and(&named)
}

/// "pause it, add Bo and close it", for a sentence.
pub(crate) fn listed(parts: &[String]) -> String {
    factory_agent::text::list_and(parts)
}

/// The card's action, read back.
fn parse(action: &str) -> Result<Action, String> {
    serde_json::from_str(action).map_err(|_| "This card's change can't be read by this version of Claktory: dismiss it.".to_owned())
}

/// Whether the change is still one to make, checked when the person
/// approves it; refused, the card stays for them to dismiss.
pub(crate) fn check(factory: &Factory, action: &str) -> Result<(), String> {
    match parse(action)? {
        Action::AutoContinue { .. } => Ok(()),
        Action::UninstallApp { app, .. } => {
            if factory.lock().config.apps.iter().any(|installed| installed.id == app) {
                Ok(())
            } else {
                Err("That app isn't installed any more, so there's nothing to uninstall: dismiss this card.".to_owned())
            }
        }
        Action::GroupPipe { pipe, group, shares, .. } => {
            let record = factory.pipes.get(&pipe).ok_or_else(|| "That pipe is gone: dismiss this card.".to_owned())?;
            if record.state == crate::pipes::State::Closed {
                return Err("That pipe is closed now: dismiss this card.".to_owned());
            }
            crate::pipes::groups::get(factory, &group).ok_or_else(|| "That group is gone: dismiss this card.".to_owned())?;
            // Only what the card showed (Fugo 31072).
            if crate::pipes::shares::of_group(factory, &group).iter().any(|(id, _)| !shares.contains(id)) {
                return Err("The group shares more websites now: dismiss this card and ask again.".to_owned());
            }
            Ok(())
        }
        Action::Pipe(change) => {
            let record = factory.pipes.get(&change.pipe).ok_or_else(|| "That pipe is gone: dismiss this card.".to_owned())?;
            if change.delete {
                if record.state != crate::pipes::State::Closed {
                    return Err("That pipe isn't closed now: dismiss this card.".to_owned());
                }
            } else if record.state == crate::pipes::State::Closed {
                return Err("That pipe is closed now: dismiss this card.".to_owned());
            }
            if change.resume && record.paused == crate::pipes::Pause::Peer {
                return Err("The other side paused the pipe since: only they can resume it. Dismiss this card.".to_owned());
            }
            change.paused_as_asked(&record)?;
            change.still_as_asked(factory, &record)?;
            let inner = factory.lock();
            let gone = change.add.iter().chain(&change.commands).chain(&change.lead).find(|id| inner.config.agent(id).is_none());
            match gone {
                Some(id) => Err(format!(
                    "{} is no longer in this Claktory, so this change can't be made: dismiss this card.",
                    change.names.get(id).unwrap_or(id)
                )),
                None => Ok(()),
            }
        }
        Action::Share { pipe, port, listener, again, .. } => {
            let record = factory.pipes.get(&pipe).ok_or_else(|| "That pipe is gone: dismiss this card.".to_owned())?;
            if let Some(why) = share_refusal(factory, &record) {
                return Err(format!("{why} Dismiss this card."));
            }
            if let Some(again) = again
                && crate::pipes::shares::on_pipe_by_port(factory, &pipe, port).is_none_or(|(id, _, ended)| id != again || !ended)
            {
                return Err("That website isn't the one the card showed any more: dismiss this card.".to_owned());
            }
            same_website(factory, port, &listener)
        }
        Action::ReleaseReader { agent, .. } => {
            release_refusal(factory, &agent).map_or(Ok(()), |why| Err(format!("{why} Dismiss this card.")))
        }
        Action::DecideReports { pipe, send, recipient, waiting } => {
            factory.pipes.get(&pipe).ok_or_else(|| "That pipe is gone: dismiss this card.".to_owned())?;
            if crate::pipes::handoff::waiting_ids(factory, &pipe) != waiting {
                return Err("Other reports wait on that pipe now than the card showed: dismiss it, and ask again.".to_owned());
            }
            if send && reports_recipient(factory, &pipe).as_deref() != Some(recipient.as_str()) {
                return Err("Who reads that pipe's reports changed since this card was asked: dismiss it, and ask again.".to_owned());
            }
            Ok(())
        }
        Action::ShowShare { pipe, share, key } => {
            let record = factory.pipes.get(&pipe).ok_or_else(|| "That pipe is gone: dismiss this card.".to_owned())?;
            if crate::pipes::approval_card::key_of(&record) != key {
                return Err("The pipe's other side changed since this card was asked: dismiss it.".to_owned());
            }
            match crate::pipes::shares::theirs_on(factory, &pipe, &share) {
                None => Err("That website isn't shared with you any more: dismiss this card.".to_owned()),
                Some((_, false)) => Err("That website isn't hidden any more: dismiss this card.".to_owned()),
                Some((_, true)) => Ok(()),
            }
        }
        Action::ApprovePipe(_) => Err(APPROVE_ON_THE_CARD.to_owned()),
        Action::TrustPipe(_) => Err(ACCEPT_ON_THE_CARD.to_owned()),
        Action::Execute(command) => crate::command_cards::check(&command),
        Action::Handoff { handoff } => match factory.errands.get(&handoff.errand) {
            Some(errand) if errand.state == v1::ErrandState::PersonsTurn as i32 => Ok(()),
            _ => Err("That errand is over: dismiss this card.".to_owned()),
        },
    }
}

/// Makes the change the person approved: what it did, or why it couldn't.
pub(crate) async fn apply(factory: &Arc<Factory>, action: &str) -> Result<String, String> {
    match parse(action)? {
        Action::AutoContinue { on } => {
            crate::usage_switch::set_auto_continue(factory, on).map_err(|error| error.to_string())?;
            Ok(if on {
                "Continue automatically is on: an agent or chat waiting on a usage limit now continues on another signed-in provider by itself, when one can run its model.".to_owned()
            } else {
                "Continue automatically is off: agents and chats waiting on a usage limit wait, unless the person continues them on another provider.".to_owned()
            })
        }
        Action::UninstallApp { app, keep_data } => {
            let name = factory.apps.manifest(&app).map_or_else(|| app.clone(), |manifest| manifest.name.clone());
            factory.uninstall_app(&v1::UninstallApp { id: app, keep_data }).await.map_err(|error| error.to_string())?;
            Ok(if keep_data {
                format!("{name} is uninstalled; what it stored is kept for a later install.")
            } else {
                format!("{name} is uninstalled, with what it stored.")
            })
        }
        Action::Pipe(change) => apply_pipe(factory, &change, None).await,
        Action::GroupPipe { pipe, group, .. } => {
            let moved = crate::pipes::groups::set(factory, &pipe, &group).map_err(|error| error.to_string())?;
            let label = factory.pipes.get(&pipe).map_or_else(|| "The pipe".to_owned(), |record| crate::pipes::handoff::label(&record));
            let websites = crate::pipes::shares::reaching(factory, &pipe);
            let group = moved.map_or_else(|| "no group".to_owned(), |group| format!("the group {}", group.name));
            Ok(if websites.is_empty() {
                format!("Moved {label} to {group}.")
            } else {
                format!("Moved {label} to {group}; it now gets {}.", listed(&websites))
            })
        }
        Action::Share { pipe, again: Some(again), .. } => {
            let name = crate::pipes::shares::share_again(factory, &again, &pipe)?;
            Ok(format!(
                "Shared {name} with the pipe again. It lasts until the person stops it, under Shared websites in the pipe's settings."
            ))
        }
        Action::ReleaseReader { agent, fresh } => {
            let request = v1::SetReportReader { agent_id: agent.clone(), reads: false, fresh };
            crate::pipes::handoff::set_reader_request(factory, &request).await.map_err(|error| match error {
                crate::factory::RequestError::Remote(text) => text,
                other => other.to_string(),
            })?;
            let name = factory.agent_name(&agent);
            Ok(if fresh {
                format!("{name} no longer reads reports: its usual tools are back, and its conversation started fresh.")
            } else {
                format!("{name} no longer reads reports: its usual tools are back, and it kept its conversation.")
            })
        }
        Action::DecideReports { pipe, send, waiting, .. } => {
            crate::pipes::handoff::decide_waiting(factory, &pipe, send)?;
            let reports = if waiting.len() == 1 { "1 report".to_owned() } else { format!("{} reports", waiting.len()) };
            Ok(if send {
                format!("Sent the {reports} that waited to their reader.")
            } else {
                format!("Discarded the {reports} that waited.")
            })
        }
        Action::ShowShare { pipe, share, .. } => {
            let name = crate::pipes::shares::theirs_on(factory, &pipe, &share).map(|(name, _)| name).unwrap_or_default();
            crate::pipes::share_listener::set_hidden(factory, &share, false)?;
            crate::pipes::line(
                factory,
                &pipe,
                format!("You showed {} again, on a card you approved.", crate::pipes::agents::escape(&name)),
            );
            Ok("It's shown again, and reachable on this computer once more.".to_owned())
        }
        Action::Share { pipe, port, name, .. } => {
            let request =
                v1::SharePipeWebsite { port: u32::from(port), name: name.clone(), pipe_ids: vec![pipe.clone()], group_id: String::new() };
            crate::pipes::shares::share(factory, &request).map_err(|error| match error {
                crate::factory::RequestError::Command(text) => text,
                other => other.to_string(),
            })?;
            let label = factory.pipes.get(&pipe).map_or_else(
                || "the pipe".to_owned(),
                |record| format!("the pipe {}", crate::pipes::handoff::label(&record).trim_start_matches("pipe ")),
            );
            Ok(format!(
                "Shared {name} (http://localhost:{port}) with {label}. It lasts until the person stops it, under Shared websites in the pipe's settings."
            ))
        }
        Action::ApprovePipe(_) => Err(APPROVE_ON_THE_CARD.to_owned()),
        Action::TrustPipe(_) => Err(ACCEPT_ON_THE_CARD.to_owned()),
        // Run by the card's answer itself, never from here.
        Action::Execute(_) => Err("A command runs only from its card's Execute.".to_owned()),
        Action::Handoff { .. } => Err("A handoff is answered on its card.".to_owned()),
    }
}

/// Makes a change to a pipe: approved by the person (`by` None), or a
/// narrowing an admin agent makes itself (`by`, its id and how the pipe
/// names it). What it did.
pub(crate) async fn apply_pipe(factory: &Arc<Factory>, change: &PipeChange, by: Option<(&str, &str)>) -> Result<String, String> {
    let (by_id, by) = (by.map(|(id, _)| id), by.map(|(_, label)| label));
    let record = factory.pipes.get(&change.pipe).ok_or_else(|| "That pipe is gone.".to_owned())?;
    change.paused_as_asked(&record)?;
    change.still_as_asked(factory, &record)?;
    let error = |error: crate::factory::RequestError| match error {
        crate::factory::RequestError::Pipe(text) => text,
        other => other.to_string(),
    };
    let named = |id: &str| factory.agent_name(id);
    let pipe_called = format!("the pipe {}", crate::pipes::handoff::label(&record).trim_start_matches("pipe "));
    let done = sentence(&change.describe(Tense::Done, &pipe_called, &named));
    // The pipe's own line for its settings: pausing, resuming and closing
    // write theirs.
    let settings_only = PipeChange { pause: false, resume: false, close: false, stop_sharing: Default::default(), ..change.clone() };
    let said = listed(&settings_only.describe(Tense::Done, "this pipe", &named));
    let members_change = !change.add.is_empty() || !change.remove.is_empty();
    let commands_change = members_change || !change.quarantine.is_empty() || !change.commands.is_empty();
    let mut request = v1::UpdatePipe { pipe_id: record.id.clone(), ..Default::default() };
    if members_change {
        let mut members: Vec<String> = record.members.iter().filter(|id| !change.remove.contains(id)).cloned().collect();
        for id in &change.add {
            if !members.contains(id) {
                members.push(id.clone());
            }
        }
        request.members = Some(v1::PipeMembers { agent_ids: members });
    }
    if commands_change {
        let members = request.members.as_ref().map_or_else(|| record.members.clone(), |members| members.agent_ids.clone());
        // Agents added can't run commands unless the change says so.
        let quarantined = members
            .iter()
            .filter(|id| {
                let was = record.quarantined.contains(id) || (change.add.contains(id) && !record.members.contains(id));
                (was || change.quarantine.contains(id)) && !change.commands.contains(id)
            })
            .cloned()
            .collect();
        request.quarantined = Some(v1::PipeMembers { agent_ids: quarantined });
    }
    request.lead_id = change.lead.clone();
    request.limits = change.limits.map(|limits| v1::PipeLimits {
        messages_per_hour: limits.messages_per_hour,
        burst: limits.burst,
        file_mb_per_day: limits.file_mb_per_day,
        ..Default::default()
    });
    request.review_outgoing = change.review_outgoing;
    request.review_incoming = change.review_incoming;
    request.name = change.name.clone();
    request.purpose = change.purpose.clone();
    request.prompt = change.instructions.clone();
    let settings = request.members.is_some()
        || request.name.is_some()
        || request.purpose.is_some()
        || request.prompt.is_some()
        || request.quarantined.is_some()
        || request.lead_id.is_some()
        || request.limits.is_some()
        || request.review_outgoing.is_some()
        || request.review_incoming.is_some();
    if settings {
        crate::pipes::update(factory, &request).map_err(error)?;
    }
    if let Some(usd) = change.spend_cap_usd {
        let tokens = crate::pipes::spend::caps(&factory.store.db(), &change.spend_agent).map(|(_, tokens)| tokens).unwrap_or_default();
        let caps = v1::SetSpendCap { agent_id: change.spend_agent.clone(), cap_usd: usd, cap_tokens: tokens };
        crate::pipes::spend::set_caps(factory, &caps).map_err(error)?;
    }
    // Approved on a card, who joined or left shows in the pipe (Rivet's own
    // narrowing says so in its line below), and new words or spending limit
    // say they came from a card (#471, Fugo's rule C).
    if by.is_none() {
        let worded = PipeChange {
            pipe: change.pipe.clone(),
            name: change.name.clone(),
            purpose: change.purpose.clone(),
            instructions: change.instructions.clone(),
            spend_cap_usd: change.spend_cap_usd,
            spend_cap_was: change.spend_cap_was,
            ..Default::default()
        };
        let worded = listed(&worded.describe(Tense::Done, "this pipe", &named));
        if !worded.is_empty() {
            crate::pipes::line(factory, &record.id, format!("You {worded}, on a card you approved."));
        }
        for id in change.add.iter().filter(|id| !record.members.contains(id)) {
            crate::pipes::line(factory, &record.id, format!("{} joined the pipe.", named(id)));
        }
        for id in change.remove.iter().filter(|id| record.members.contains(id)) {
            crate::pipes::line(factory, &record.id, format!("{} left the pipe.", named(id)));
        }
    }
    if let Some(liaison) = &change.liaison {
        let request = v1::AddPipeLiaison { pipe_id: record.id.clone(), name: liaison.clone() };
        crate::pipes::add_liaison(factory, &request).await.map_err(error)?;
    }
    let state = |action: v1::PipeAction| v1::SetPipeState { pipe_id: record.id.clone(), action: action.into() };
    if change.pause {
        crate::pipes::set_pipe_state_by(factory, &state(v1::PipeAction::Pause), by).map_err(error)?;
    }
    if change.resume {
        crate::pipes::set_pipe_state_by(factory, &state(v1::PipeAction::Resume), by).map_err(error)?;
    }
    if (settings || change.spend_cap_usd.is_some())
        && let Some(by) = by
    {
        crate::pipes::line(factory, &record.id, format!("{by} {said}."));
    }
    // Each website it stops sharing, cut at once and said in the pipe as
    // whose (#471 G4, Fugo: "labelled as Rivet's").
    for id in change.stop_sharing.keys() {
        let said = |name: &str| {
            let name = crate::pipes::agents::escape(name);
            match by {
                Some(by) => format!("{by} stopped sharing {name} with this pipe."),
                None => format!("You stopped sharing {name} with this pipe, on a card you approved."),
            }
        };
        // One stopped meanwhile (by the person, say) is stopped already.
        if let Err(why) = crate::pipes::shares::stop_on(factory, &record.id, id, by_id, said) {
            tracing::debug!(%why, "a website to stop sharing wasn't shared");
        }
    }
    if change.close {
        crate::pipes::set_pipe_state_by(factory, &state(v1::PipeAction::Close), by).map_err(error)?;
    }
    if change.delete {
        crate::pipes::set_pipe_state_by(factory, &state(v1::PipeAction::Delete), by).map_err(error)?;
    }
    factory.broadcast_workspace(&factory.lock());
    Ok(format!("{done}."))
}

/// What the agent that asked is told once the person decided: a summary,
/// and the event's text. `made`: how making it went, approved; None when it
/// wasn't (dismissed, or a reply). `trust`: a pipe's trust card, accepted
/// rather than approved.
pub(crate) fn told(
    person: &str,
    title: &str,
    comment: &str,
    replied: bool,
    trust: bool,
    made: Option<Result<String, String>>,
) -> (String, String) {
    let approving = if trust { "accepting" } else { "approving" };
    let their = if comment.is_empty() { String::new() } else { format!("\n\nTheir comment:\n\n{comment}") };
    match made {
        Some(Ok(done)) => (format!("{person} approved \"{title}\"."), format!("{person} approved \"{title}\". {done}{their}")),
        Some(Err(why)) => {
            (format!("\"{title}\" couldn't be done."), format!("{person} approved \"{title}\", but it couldn't be done: {why}{their}"))
        }
        None if replied => (
            format!("{person} replied to \"{title}\"."),
            format!("{person} replied to \"{title}\" instead of {approving} it, so nothing was changed:\n\n{comment}"),
        ),
        None => (format!("{person} dismissed \"{title}\"."), format!("{person} dismissed \"{title}\", so nothing was changed.{their}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reply to a trust card says it wasn't accepted; any other card's,
    /// approved (Zucchero's #471 nit 4).
    #[test]
    fn a_reply_says_what_the_card_asked() {
        let (_, trust) = told("Matt", "Trust the Claktory", "Who is it?", true, true, None);
        assert!(trust.starts_with("Matt replied to \"Trust the Claktory\" instead of accepting it, so nothing was changed"), "{trust}");
        let (_, other) = told("Matt", "Add Cy", "Bo instead", true, false, None);
        assert!(other.contains("instead of approving it"), "{other}");
    }

    /// A /proc of its own (W10): TCP tables with `sockets` (port, uid,
    /// inode, listening), and processes, each with its pid, command, start
    /// and the socket inodes it holds.
    fn fake_proc(sockets: &[(u16, u32, u64, bool)], processes: &[(u32, &str, u64, &[u64])]) -> std::path::PathBuf {
        static MADE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let proc = std::env::temp_dir().join(format!("factoryd-proc-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&proc);
        std::fs::create_dir_all(proc.join("net")).expect("net");
        let header = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
        let mut v4 = header.to_owned();
        let mut v6 = header.to_owned();
        for (at, (port, uid, inode, listening)) in sockets.iter().enumerate() {
            let state = if *listening { "0A" } else { "01" };
            if at % 2 == 0 {
                v4.push_str(&format!("   {at}: 0100007F:{port:04X} 00000000:0000 {state} 00000000:00000000 00:00000000 00000000  {uid}        0 {inode} 1 0000000000000000 100 0 0 10 0\n"));
            } else {
                v6.push_str(&format!("   {at}: 00000000000000000000000001000000:{port:04X} 00000000000000000000000000000000:0000 {state} 00000000:00000000 00:00000000 00000000  {uid}        0 {inode} 1 0000000000000000 100 0 0 10 0\n"));
            }
        }
        std::fs::write(proc.join("net/tcp"), v4).expect("tcp");
        std::fs::write(proc.join("net/tcp6"), v6).expect("tcp6");
        for (pid, command, started, inodes) in processes {
            let dir = proc.join(pid.to_string());
            std::fs::create_dir_all(dir.join("fd")).expect("fd");
            std::fs::write(dir.join("comm"), format!("{command}\n")).expect("comm");
            std::fs::write(dir.join("stat"), format!("{pid} ({command} (x)) S 1 1 1 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 {started} 1000 10\n")).expect("stat");
            std::os::unix::fs::symlink("/dev/null", dir.join("fd/0")).expect("fd 0");
            for (at, inode) in inodes.iter().enumerate() {
                std::os::unix::fs::symlink(format!("socket:[{inode}]"), dir.join(format!("fd/{}", at + 3))).expect("socket fd");
            }
        }
        proc
    }

    #[test]
    fn lsof_names_the_listener_a_shared_website_reaches_first() {
        let named = |text: &str| from_lsof_fields(text).map(|listener| (listener.pid, listener.command));
        // Abbacchio's row 1339: python on *:8790 over IPv6, another on
        // 127.0.0.1: the share reaches the second.
        let both = "p100\ncpython3\nf4\ntIPv6\nn*:8790\np200\ncpython3.12\nf3\ntIPv4\nn127.0.0.1:8790\n";
        assert_eq!(named(both), Some((200, "python3.12".into())));
        // Alone, the IPv6 wildcard is it (dual-stack, 127.0.0.1 reaches it).
        assert_eq!(named("p100\ncpython3\nf4\ntIPv6\nn*:8790\n"), Some((100, "python3".into())));
        // A v4 wildcard before [::1]; one process with both sockets once.
        assert_eq!(named("p7\ncvite\nf20\ntIPv6\nn[::1]:5173\np8\ncnode\nf21\ntIPv4\nn*:5173\n"), Some((8, "node".into())));
        assert_eq!(named("p7\ncvite\nf20\ntIPv6\nn[::1]:5173\nf21\ntIPv4\nn127.0.0.1:5173\n"), Some((7, "vite".into())));
        // Listening only on the network's address: never reached, not named.
        assert_eq!(named("p9\ncserver\nf3\ntIPv4\nn192.168.1.5:8080\n"), None);
        assert_eq!(named(""), None);
    }

    #[test]
    fn the_listener_is_named_from_proc_by_the_socket_it_holds_and_another_users_never() {
        let me = 1000;
        // The person's vite holds the listening socket; another of theirs
        // holds a connection on the same port, and an older one nothing.
        let proc = fake_proc(
            &[(3000, me, 777, true), (3000, me, 778, false), (8080, 0, 900, true)],
            &[(41, "node", 5, &[778]), (42, "vite", 123456, &[777]), (43, "bash", 7, &[])],
        );
        assert_eq!(from_proc(&proc, 3000, me), Some(Heard::Mine(Listener { pid: 42, command: "vite".into(), started: Some(123456) })));
        // root's docker-proxy on 8080: whose, never which.
        assert_eq!(from_proc(&proc, 8080, me), Some(Heard::Others("root".into())));
        // Nothing listening there.
        assert_eq!(from_proc(&proc, 5173, me), Some(Heard::Unknown));
        // Another user's socket beside the person's, on IPv6: theirs.
        let both = fake_proc(&[(3000, me, 777, true), (3000, 4242, 779, true)], &[(42, "vite", 9, &[777])]);
        assert!(matches!(from_proc(&both, 3000, me), Some(Heard::Others(_))));
        // The person's socket held by no process it can read: unknown.
        let hidden = fake_proc(&[(3000, me, 777, true)], &[(43, "bash", 7, &[])]);
        assert_eq!(from_proc(&hidden, 3000, me), Some(Heard::Unknown));
        // Two of the person's programs on the port, one on [::1] and one on
        // 127.0.0.1: the one a shared website reaches first, whichever
        // /proc lists first (Abbacchio row 1339).
        let two = fake_proc(&[(3000, me, 1, false), (3000, me, 701, true), (3000, me, 702, true)], &[(50, "v6", 1, &[701]), (60, "v4", 2, &[702])]);
        assert_eq!(from_proc(&two, 3000, me), Some(Heard::Mine(Listener { pid: 60, command: "v4".into(), started: Some(2) })));
        let _ = std::fs::remove_dir_all(two);
        // No tables (not Linux): lsof's turn.
        assert_eq!(from_proc(&std::env::temp_dir().join("factoryd-no-proc"), 3000, me), None);
        for dir in [proc, both, hidden] {
            let _ = std::fs::remove_dir_all(dir);
        }

        // At Approve: the same pid and command, and start when both know it.
        let shown = Listener { pid: 42, command: "vite".into(), started: Some(5) };
        assert!(Listener { started: Some(5), ..shown.clone() }.is(&shown));
        assert!(!Listener { started: Some(6), ..shown.clone() }.is(&shown), "a pid given to another program since");
        assert!(!Listener { command: "node".into(), ..shown.clone() }.is(&shown));
        assert!(Listener { started: Some(6), ..shown.clone() }.is(&Listener { started: None, ..shown.clone() }), "a card from before");
        // A card from before keeps reading.
        let old: Listener = serde_json::from_str(r#"{"pid": 42, "command": "vite"}"#).expect("old card");
        assert_eq!(old.started, None);
    }

    fn named(id: &str) -> String {
        let mut name = id.to_owned();
        name[..1].make_ascii_uppercase();
        name
    }

    #[test]
    fn a_change_reads_as_asked_and_as_done() {
        // Abbacchio's LOWs (row 1054).
        let pipe = "the pipe \"Launch\"";
        let resume = PipeChange { resume: true, ..Default::default() };
        assert_eq!(listed(&resume.describe(Tense::Ask, pipe, &named)), "resume the pipe \"Launch\"");
        let both = PipeChange { pause: true, add: vec!["mia".into()], ..Default::default() };
        assert_eq!(listed(&both.describe(Tense::Ask, pipe, &named)), "pause the pipe \"Launch\" and add Mia to it");
        let pause = PipeChange { pause: true, ..Default::default() };
        assert_eq!(sentence(&pause.describe(Tense::Done, pipe, &named)), "Paused the pipe \"Launch\"");
        let out = PipeChange { remove: vec!["lea".into()], ..Default::default() };
        assert_eq!(sentence(&out.describe(Tense::Done, pipe, &named)), "Took Lea out of the pipe \"Launch\"");
        // Named as it was asked, once that agent is gone.
        let kept = PipeChange { add: vec!["mia".into()], names: [("mia".into(), "Mía".into())].into(), ..Default::default() };
        assert_eq!(listed(&kept.describe(Tense::Ask, pipe, &|id| id.to_owned())), "add Mía to the pipe \"Launch\"");
    }
}
