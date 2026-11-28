//! The source's side of `claktory-move/1` (#217, phase 2; docs/MOVE.md,
//! "The handshake"): moving this Claktory to the computer that shows a
//! receive code (`factoryd receive` there, `move_import::receive`).
//!
//! - `Mover` holds the move's own endpoint, from its own key in
//!   `<home>/moving/key`, never the tunnel's identity, so every reconnect
//!   is the same source.
//! - `Link` is one connection: pairing (the six digits), the plan and the
//!   target's checks, rounds (the tree, listed with its blocks' hashes; the
//!   target asks for the blocks it lacks), import's progress, the commit.
//! - `factoryd move CODE` drives it from a terminal: rounds while Claktory
//!   runs, until they settle; then it stops Claktory, sends the last round,
//!   waits for the target to import it, fences this copy and commits. Any
//!   failure before the commit starts Claktory again, as it was.

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::mpsc;

use crate::encoding::{hex, unhex};
use crate::home::Home;
use crate::move_export::{self, ItemKind, Options, Place, Tree, size_text};
use crate::move_wire::{self, Entry, Frame, Kind, ReceiveCode};

/// How long dialing the target may take (less in tests, where it's on
/// this computer and some aren't there to answer).
const DIAL: Duration = if cfg!(test) { Duration::from_secs(5) } else { Duration::from_secs(30) };
/// How long the target has to answer, within a step.
const ANSWER: Duration = Duration::from_secs(120);
/// How long the person on the target has to confirm the digits, and import
/// has between two words of progress.
const PATIENCE: Duration = Duration::from_secs(30 * 60);
/// Pre-copy has settled after a round that sends less than this…
pub(crate) const SETTLED_BYTES: u64 = 64 << 20;
/// …or takes less than this…
pub(crate) const SETTLED_TIME: Duration = Duration::from_secs(30);
/// …or after this many rounds.
pub(crate) const MAX_ROUNDS: u32 = 5;
/// How long a dropped connection is dialed again before the move gives up
/// (before the commit; after it, it never does).
pub(crate) const REDIAL: Duration = Duration::from_secs(120);
/// While the target imports, how long dialing it again may go without any
/// answer (its receive stopped, say) before the move gives up (friend bug
/// #22): one that answers, even to turn this side away for now, gets
/// `PATIENCE`.
const UNHEARD: Duration = if cfg!(test) { Duration::from_secs(2) } else { REDIAL };
/// A file changed this recently may change again within its mtime's
/// precision: its hashes aren't kept for the next round.
const RACY_MS: i64 = 2000;

/// Why a step didn't go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Problem {
    /// The connection broke or went quiet: dialing again may carry on.
    Lost(String),
    /// It can't go on, in a sentence for the person (the target's own, or
    /// this side's).
    Said(String),
    /// The target has the last round imported already (a frozen move that
    /// restarted after it): it said `ready` instead of answering.
    Ready,
}

impl Problem {
    /// A sentence for the person.
    pub(crate) fn text(&self, machine: &str) -> String {
        match self {
            Problem::Lost(detail) => format!("The connection to {machine} was lost ({detail})."),
            Problem::Said(why) => why.clone(),
            Problem::Ready => format!("{machine} answered out of turn: it's ready already."),
        }
    }
}

fn lost(error: impl std::fmt::Display) -> Problem {
    Problem::Lost(error.to_string())
}

/// A target that can't be reached again, in a sentence for the person
/// (friend bug #22): its receive has likely stopped.
pub(crate) fn unanswered(machine: &str, problem: Problem) -> Problem {
    match problem {
        Problem::Lost(_) => Problem::Said(format!(
            "{machine} stopped answering, so factoryd receive has likely stopped there. Check that it runs, and start the move again."
        )),
        problem => problem,
    }
}

/// `<home>/moving`: the move's own key, its staging and (later) its state.
pub(crate) fn moving_dir(home: &Home) -> PathBuf {
    home.root().join("moving")
}

/// The endpoint id this Claktory's move dials from (its key, made now if
/// there's none yet): what it names taking a move with the account, the
/// only one the other computer lets in.
pub(crate) fn endpoint_id(home: &Home) -> Result<String, String> {
    key(home).map(|key| key.public().to_string())
}

/// The move's key, made the first time: one source for the whole move.
fn key(home: &Home) -> Result<SecretKey, String> {
    let dir = moving_dir(home);
    let path = dir.join("key");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let bytes: [u8; 32] = unhex(text.trim())
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or_else(|| format!("{} is damaged: delete it and try again.", path.display()))?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let key = SecretKey::generate();
            crate::remote::private_dir(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
            crate::remote::write_private(&path, format!("{}\n", hex(&key.to_bytes())).as_bytes())
                .map_err(|error| format!("{}: {error}", path.display()))?;
            Ok(key)
        }
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// The target, as it said in `welcome`, and the digits both show.
#[derive(Clone, Debug, Default)]
pub(crate) struct Target {
    pub(crate) machine: String,
    pub(crate) version: String,
    pub(crate) os: String,
    pub(crate) arch: String,
    pub(crate) home: String,
    pub(crate) user_home: String,
    pub(crate) digits: String,
    /// The digits were confirmed on an earlier connection.
    pub(crate) resumed: bool,
    /// The address this computer reached it at, when directly ("203.0.113.7:
    /// 4433"); empty through a relay. Seen here, not said by the target.
    pub(crate) address: String,
}

/// What the target said about the plan.
#[derive(Clone, Debug, Default)]
pub(crate) struct Checked {
    pub(crate) problems: Vec<String>,
    pub(crate) map: Vec<(String, String)>,
    pub(crate) missing: Vec<String>,
}

/// What a round did.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Rounded {
    /// Block bytes sent.
    pub(crate) sent: u64,
    /// What the target has of the tree, and the tree's size.
    pub(crate) have: u64,
    pub(crate) total: u64,
    /// Blocks that had changed since they were listed.
    pub(crate) stale: u32,
}

/// A file's hashes, kept between rounds while it doesn't change.
#[derive(Clone, Debug)]
struct Hashed {
    size: u64,
    mtime_ms: i64,
    ino: u64,
    ctime_ns: i64,
    block: u64,
    hashes: Vec<String>,
}

/// One path of a round's listing, and where its content is read.
#[derive(Clone, Debug)]
pub(crate) struct Listed {
    pub(crate) entry: Entry,
    pub(crate) source: Option<PathBuf>,
}

/// The move from this side: its endpoint, where it dials, and the hashes
/// it keeps between rounds.
pub(crate) struct Mover {
    pub(crate) code: ReceiveCode,
    endpoint: Endpoint,
    hashes: HashMap<PathBuf, Hashed>,
}

impl Mover {
    /// The move's endpoint, from `<home>/moving/key`. Refuses a target older
    /// than this Claktory before dialing it.
    pub(crate) async fn new(home: &Home, code: ReceiveCode) -> Result<Mover, String> {
        if !code.version.is_empty() && crate::update::newer(crate::update::VERSION, &code.version) {
            return Err(format!(
                "{} has Claktory {}, older than this one ({}). Update Claktory there, run factoryd receive again, and use its new code.",
                code.machine,
                code.version,
                crate::update::VERSION
            ));
        }
        let key = key(home)?;
        let relays: Vec<RelayUrl> = code.relays.iter().filter_map(|relay| relay.parse().ok()).collect();
        let relay_mode = if relays.is_empty() { RelayMode::Disabled } else { RelayMode::Custom(relays.into_iter().collect::<RelayMap>()) };
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(key)
            .relay_mode(relay_mode)
            .bind()
            .await
            .map_err(|error| format!("The move's connection couldn't start: {error}"))?;
        Ok(Mover { code, endpoint, hashes: HashMap::new() })
    }

    /// Closes the endpoint once its connections have said goodbye, so the
    /// target hears the move hang up rather than waiting for it to time out.
    pub(crate) async fn close(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(5), self.endpoint.close()).await;
    }

    /// This side's id, as the target sees it.
    pub(crate) fn id(&self) -> String {
        self.endpoint.id().to_string()
    }

    /// Dials the target and says hello; its welcome, or why not.
    pub(crate) async fn connect(&self) -> Result<Link, Problem> {
        let id: EndpointId = self.code.endpoint.parse().map_err(|_| Problem::Said("That receive code is damaged.".to_owned()))?;
        let mut addr = EndpointAddr::new(id);
        for relay in &self.code.relays {
            if let Ok(url) = relay.parse::<RelayUrl>() {
                addr = addr.with_relay_url(url);
            }
        }
        for address in &self.code.addresses {
            addr = addr.with_ip_addr(*address);
        }
        let connection = match tokio::time::timeout(DIAL, self.endpoint.connect(addr, move_wire::ALPN)).await {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => return Err(Problem::Lost(format!("{error:#}"))),
            Err(_) => return Err(Problem::Lost("no answer in time".to_owned())),
        };
        let (send, recv) = connection.open_bi().await.map_err(lost)?;
        let mut link = Link::new(connection, send, recv);
        let hello = Frame::Hello {
            secret: self.code.secret.clone(),
            version: crate::update::VERSION.to_owned(),
            machine: crate::remote::machine_name().to_owned(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
        };
        link.say(hello, Vec::new()).await?;
        match link.next(ANSWER).await? {
            (Frame::Welcome { version, machine, os, arch, home, user_home, resumed, .. }, _) => {
                link.target = Target {
                    machine: if machine.is_empty() { self.code.machine.clone() } else { machine },
                    version,
                    os,
                    arch,
                    home,
                    user_home,
                    digits: move_wire::digits(&self.code.endpoint, &self.id(), &self.code.secret),
                    resumed,
                    address: link.address(),
                };
                Ok(link)
            }
            (other, _) => Err(unexpected(&other)),
        }
    }

    /// Dials again after a lost connection, until `within` passes; the
    /// target must know this move (`resumed`). Its refusals are tried again
    /// too: it turns a second connection away until it has closed the one
    /// that dropped.
    pub(crate) async fn redial(&self, within: Duration) -> Result<Link, Problem> {
        self.redial_heard(within, within).await
    }

    /// As `redial`, but gives up sooner, once nothing has answered at all
    /// for `unheard`: a target that answers, even to turn this side away
    /// for now, is still there; one that doesn't has likely stopped.
    pub(crate) async fn redial_heard(&self, within: Duration, unheard: Duration) -> Result<Link, Problem> {
        let started = Instant::now();
        let deadline = started + within;
        let mut heard = started;
        let mut wait = Duration::from_secs(1);
        loop {
            match self.connect().await {
                Ok(link) if link.target.resumed => return Ok(link),
                Ok(mut link) => {
                    let why = format!(
                        "{} no longer knows this move. Run factoryd receive there again, and start the move again with its code.",
                        link.target.machine
                    );
                    link.fail(&why).await;
                    return Err(Problem::Said(why));
                }
                Err(problem @ (Problem::Lost(_) | Problem::Said(_))) => {
                    let now = Instant::now();
                    if matches!(problem, Problem::Said(_)) {
                        heard = now;
                    }
                    if now + wait >= deadline || now + wait >= heard + unheard {
                        return Err(problem);
                    }
                    tracing::debug!(?problem, "the move's target doesn't take the connection yet");
                    tokio::time::sleep(wait).await;
                    wait = (wait * 2).min(Duration::from_secs(10));
                }
                Err(problem) => return Err(problem),
            }
        }
    }

    /// Lists the tree for a round: every item, with each file's blocks'
    /// hashes. Hashes of the Claktory's own files are kept for the next
    /// round while they don't change; a file gone meanwhile is left out.
    pub(crate) async fn list(&mut self, tree: &Tree) -> Result<Vec<Listed>, String> {
        let items = tree.items.clone();
        let mut hashes = std::mem::take(&mut self.hashes);
        let (listing, hashes) = tokio::task::spawn_blocking(move || {
            let listing = list(&items, &mut hashes);
            (listing, hashes)
        })
        .await
        .map_err(|error| error.to_string())?;
        self.hashes = hashes;
        listing
    }

    /// Forgets a file's hashes: it changed under the listing.
    fn forget(&mut self, source: &Path) {
        self.hashes.remove(source);
    }
}

fn list(items: &[move_export::Item], hashes: &mut HashMap<PathBuf, Hashed>) -> Result<Vec<Listed>, String> {
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let now_ms = crate::transcript::now_ms();
    let mut listing = Vec::with_capacity(items.len());
    for item in items {
        let entry = Entry { path: item.path.clone(), ..Entry::default() };
        match &item.kind {
            ItemKind::Dir { mode } => listing.push(Listed { entry: Entry { kind: Kind::Dir, mode: *mode, ..entry }, source: None }),
            ItemKind::Symlink { target } => {
                listing.push(Listed { entry: Entry { kind: Kind::Symlink, target: target.clone(), ..entry }, source: None });
            }
            ItemKind::File { source, live } => {
                let meta = match std::fs::metadata(source) {
                    Ok(meta) => meta,
                    Err(error) if *live && error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(format!("{}: {error}", source.display())),
                };
                let size = meta.len();
                let mtime_ms =
                    meta.modified().ok().and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |at| at.as_millis() as i64);
                #[cfg(unix)]
                let (ino, ctime_ns, mode) =
                    (meta.ino(), meta.ctime().saturating_mul(1_000_000_000).saturating_add(meta.ctime_nsec()), meta.permissions().mode() & 0o777);
                // A name that can't be read again counts as changed: its hashes are made again.
                #[cfg(windows)]
                let (ino, ctime_ns, mode) = (
                    crate::windows::fs::identity(source).map_or(0, |identity| identity.index),
                    crate::windows::fs::changed_ns(source).unwrap_or(-1),
                    move_export::permissions(&meta),
                );
                let block = move_wire::block_size(size);
                let kept = hashes.get(source).filter(|kept| {
                    *live
                        && kept.size == size
                        && kept.mtime_ms == mtime_ms
                        && kept.ino == ino
                        && kept.ctime_ns == ctime_ns
                        && kept.block == block
                });
                let file_hashes = match kept {
                    Some(kept) => kept.hashes.clone(),
                    None => match move_wire::hash_file(source, block) {
                        Ok(file_hashes) => file_hashes,
                        Err(error) if *live && error.kind() == io::ErrorKind::NotFound => continue,
                        Err(error) => return Err(format!("{}: {error}", source.display())),
                    },
                };
                // Kept only when it's had time to settle: a change within the
                // same millisecond and size would otherwise go unseen.
                if *live && mtime_ms < now_ms - RACY_MS {
                    hashes.insert(source.clone(), Hashed { size, mtime_ms, ino, ctime_ns, block, hashes: file_hashes.clone() });
                } else {
                    hashes.remove(source);
                }
                listing.push(Listed {
                    entry: Entry {
                        kind: Kind::File,
                        size,
                        mode,
                        mtime_ms,
                        block,
                        hashes: file_hashes,
                        ..entry
                    },
                    source: Some(source.clone()),
                });
            }
        }
    }
    Ok(listing)
}

fn unexpected(frame: &Frame) -> Problem {
    Problem::Said(format!("The other computer answered out of turn ({frame:?}). Start the move again."))
}

async fn say(out: Option<&mpsc::Sender<(Frame, Vec<u8>)>>, frame: Frame, bytes: Vec<u8>) -> Result<(), Problem> {
    let Some(out) = out else { return Err(Problem::Lost("the connection was closed".to_owned())) };
    out.send((frame, bytes)).await.map_err(|_| Problem::Lost("the connection broke".to_owned()))
}

async fn next(frames: &mut mpsc::Receiver<Result<(Frame, Vec<u8>), Problem>>, wait: Duration) -> Result<(Frame, Vec<u8>), Problem> {
    match tokio::time::timeout(wait, frames.recv()).await {
        Ok(Some(Ok((Frame::Failed { why }, _)))) => Err(Problem::Said(why)),
        Ok(Some(read)) => read,
        Ok(None) => Err(Problem::Lost("the connection broke".to_owned())),
        Err(_) => Err(Problem::Lost("no answer in time".to_owned())),
    }
}

/// One connection to the target. Frames go through a writer task and come
/// through a reader task, so any step can be dropped part way (a cancel, a
/// freeze) without cutting a frame in half.
pub(crate) struct Link {
    connection: Connection,
    out: Option<mpsc::Sender<(Frame, Vec<u8>)>>,
    writer: Option<tokio::task::JoinHandle<()>>,
    frames: mpsc::Receiver<Result<(Frame, Vec<u8>), Problem>>,
    pub(crate) target: Target,
}

/// Frames waiting to be written: a few blocks at most.
const QUEUED: usize = 4;

impl Link {
    /// The address the connection reaches the target at now, when it goes
    /// directly; empty through a relay.
    pub(crate) fn address(&self) -> String {
        let paths = self.connection.paths();
        let path = paths.iter().find(|path| path.is_selected()).or_else(|| paths.iter().next());
        match path.map(|path| path.remote_addr().clone()) {
            Some(iroh::TransportAddr::Ip(address)) => address.to_string(),
            _ => String::new(),
        }
    }

    fn new(connection: Connection, mut send: SendStream, mut recv: RecvStream) -> Link {
        let (out, mut queued) = mpsc::channel::<(Frame, Vec<u8>)>(QUEUED);
        let writer = tokio::spawn(async move {
            while let Some((frame, bytes)) = queued.recv().await {
                let wait = ANSWER + Duration::from_secs(bytes.len() as u64 / 1_000_000);
                match tokio::time::timeout(wait, move_wire::write(&mut send, &frame, &bytes)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::debug!(%error, "the move's connection stopped taking frames");
                        return;
                    }
                    Err(_) => {
                        tracing::debug!("the other computer stopped reading the move's frames");
                        return;
                    }
                }
            }
            let _ = send.finish();
        });
        let (heard, frames) = mpsc::channel(64);
        tokio::spawn(async move {
            loop {
                let read = move_wire::read(&mut recv).await.map_err(lost);
                let ended = read.is_err();
                if matches!(read, Ok((Frame::Unknown, _))) {
                    continue;
                }
                if heard.send(read).await.is_err() || ended {
                    return;
                }
            }
        });
        Link { connection, out: Some(out), writer: Some(writer), frames, target: Target::default() }
    }

    async fn say(&mut self, frame: Frame, bytes: Vec<u8>) -> Result<(), Problem> {
        say(self.out.as_ref(), frame, bytes).await
    }

    /// The next frame: `failed` becomes its sentence.
    async fn next(&mut self, wait: Duration) -> Result<(Frame, Vec<u8>), Problem> {
        next(&mut self.frames, wait).await
    }

    /// This side's person confirmed the digits: says so.
    pub(crate) async fn send_confirmed(&mut self) -> Result<(), Problem> {
        self.say(Frame::Confirmed {}, Vec::new()).await
    }

    /// Waits for the target's person to confirm the digits.
    pub(crate) async fn target_confirmed(&mut self) -> Result<(), Problem> {
        match self.next(PATIENCE).await? {
            (Frame::Confirmed {}, _) => Ok(()),
            (other, _) => Err(unexpected(&other)),
        }
    }

    /// This side's person confirmed the digits: says so, and waits for the
    /// target's person. The tests' whole move (`drive`); the app's waits on
    /// its own (`move_run`).
    #[cfg(test)]
    pub(crate) async fn confirm(&mut self) -> Result<(), Problem> {
        self.send_confirmed().await?;
        self.target_confirmed().await
    }

    /// Tells the target the move is off before it was confirmed, or failed
    /// here; never waits long.
    pub(crate) async fn fail(&mut self, why: &str) {
        self.last_word(Frame::Failed { why: why.to_owned() }).await;
    }

    /// Calls the move off: the target throws away what it has.
    pub(crate) async fn cancel(&mut self, why: &str) {
        self.last_word(Frame::Cancel { why: why.to_owned() }).await;
    }

    /// Sends `frame` after what's queued, then hangs up.
    async fn last_word(&mut self, frame: Frame) {
        if let Some(out) = self.out.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), out.send((frame, Vec::new()))).await;
        }
        self.close().await;
    }

    /// Hangs up once what's queued is written, giving it a moment to arrive.
    pub(crate) async fn close(&mut self) {
        self.out = None;
        if let Some(writer) = self.writer.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), writer).await;
        }
        let _ = tokio::time::timeout(Duration::from_secs(2), self.frames.recv()).await;
        self.connection.close(0u32.into(), b"done");
    }

    /// Sends what the move means; the target's checks.
    pub(crate) async fn plan(&mut self, tree: &Tree, map: &[(String, String)], tools: &[String]) -> Result<Checked, Problem> {
        let manifest = serde_json::to_value(&tree.manifest).map_err(|error| Problem::Said(error.to_string()))?;
        self.say(Frame::Plan { manifest, map: map.to_vec(), tools: tools.to_vec() }, Vec::new()).await?;
        loop {
            match self.next(ANSWER).await? {
                (Frame::Checked { problems, map, missing, .. }, _) => return Ok(Checked { problems, map, missing }),
                (Frame::Progress { .. }, _) => {}
                (Frame::Ready { .. }, _) => return Err(Problem::Ready),
                (other, _) => return Err(unexpected(&other)),
            }
        }
    }

    /// One round: the listing, while the target says what it needs; then
    /// those blocks; then what it has. `progress` hears what the target has
    /// of the listing, and its size, as blocks go.
    pub(crate) async fn round(
        &mut self,
        mover: &mut Mover,
        n: u32,
        last: bool,
        listing: &[Listed],
        progress: &mut (dyn FnMut(u64, u64) + Send),
    ) -> Result<Rounded, Problem> {
        let Link { out, frames, .. } = self;
        let out = out.as_ref();
        let listing_out = async {
            say(out, Frame::Round { n, last }, Vec::new()).await?;
            for listed in listing {
                say(out, Frame::Entry(listed.entry.clone()), Vec::new()).await?;
            }
            say(out, Frame::RoundEnd { n }, Vec::new()).await
        };
        let needs_in = async {
            let mut needs: Vec<(String, Vec<u32>)> = Vec::new();
            loop {
                match next(frames, ANSWER).await? {
                    (Frame::Need { path, blocks }, _) => needs.push((path, blocks)),
                    (Frame::NeedEnd { n: end }, _) if end == n => return Ok(needs),
                    (Frame::Progress { .. }, _) => {}
                    (Frame::Ready { .. }, _) => return Err(Problem::Ready),
                    (other, _) => return Err(unexpected(&other)),
                }
            }
        };
        let (out, needs) = tokio::join!(listing_out, needs_in);
        // The target's own word first: it explains why the listing broke off.
        let needs = needs?;
        out?;
        let by_path: HashMap<&str, &Listed> = listing.iter().map(|listed| (listed.entry.path.as_str(), listed)).collect();
        // What the target has already: all of it but the blocks it asked for.
        let files = listing.iter().filter(|listed| listed.entry.kind == Kind::File);
        let total: u64 = files.map(|listed| listed.entry.size).sum();
        let asked: u64 = needs
            .iter()
            .filter_map(|(path, blocks)| Some((by_path.get(path.as_str())?, blocks)))
            .map(|(listed, blocks)| {
                let entry = &listed.entry;
                blocks.iter().map(|&index| entry.block.min(entry.size.saturating_sub(u64::from(index) * entry.block))).sum::<u64>()
            })
            .sum();
        let had = total.saturating_sub(asked);
        progress(had, total);
        let mut rounded = Rounded::default();
        for (path, blocks) in needs {
            let Some(listed) = by_path.get(path.as_str()).filter(|listed| listed.entry.kind == Kind::File) else {
                let why = format!("The other computer asked for {path}, which this move doesn't have.");
                self.fail(&why).await;
                return Err(Problem::Said(why));
            };
            let Some(source) = &listed.source else { continue };
            let entry = &listed.entry;
            let mut file = tokio::fs::File::open(source).await.ok();
            for index in blocks {
                let start = u64::from(index) * entry.block;
                let Some(hash) = entry.hashes.get(index as usize) else {
                    let why = format!("The other computer asked for a part of {path} that doesn't exist.");
                    self.fail(&why).await;
                    return Err(Problem::Said(why));
                };
                let want = entry.block.min(entry.size.saturating_sub(start));
                // What's there now: a file that shrank or went sends less,
                // which the target sees as changed.
                let mut bytes = Vec::with_capacity(want as usize);
                if let Some(file) = file.as_mut()
                    && file.seek(io::SeekFrom::Start(start)).await.is_ok()
                {
                    let _ = file.take(want).read_to_end(&mut bytes).await;
                }
                if move_wire::block_hash(&bytes) != *hash {
                    mover.forget(source);
                }
                let size = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
                let length = bytes.len() as u64;
                self.say(Frame::Block { path: path.clone(), index, size }, bytes).await?;
                rounded.sent += length;
                progress((had + rounded.sent).min(total), total);
            }
        }
        self.say(Frame::BlocksEnd { n }, Vec::new()).await?;
        loop {
            match self.next(ANSWER).await? {
                (Frame::RoundDone { n: done, bytes_have, bytes_total, stale }, _) if done == n => {
                    rounded.have = bytes_have;
                    rounded.total = bytes_total;
                    rounded.stale = stale;
                    return Ok(rounded);
                }
                (Frame::Progress { .. }, _) => {}
                (other, _) => return Err(unexpected(&other)),
            }
        }
    }

    /// After the last round: import's progress, until the target is ready.
    pub(crate) async fn ready(&mut self, progress: &mut (dyn FnMut(&str, &str) + Send)) -> Result<(), Problem> {
        loop {
            match self.next(PATIENCE).await? {
                (Frame::Ready { .. }, _) => return Ok(()),
                (Frame::Progress { step, text }, _) => progress(&step, &text),
                (other, _) => return Err(unexpected(&other)),
            }
        }
    }

    /// The commit: this copy has moved away. Ok once the target recorded it.
    pub(crate) async fn commit(&mut self, at_ms: i64, export_id: &str) -> Result<(), Problem> {
        self.say(Frame::Commit { at_ms, export_id: export_id.to_owned() }, Vec::new()).await?;
        loop {
            match self.next(ANSWER).await? {
                (Frame::Done {}, _) => {
                    self.out = None;
                    self.connection.close(0u32.into(), b"done");
                    return Ok(());
                }
                (Frame::Progress { .. } | Frame::Ready { .. }, _) => {}
                (other, _) => return Err(unexpected(&other)),
            }
        }
    }
}

/// Where folders go: this computer's home folder to the target's, and this
/// Claktory's home to its; the longer first.
pub(crate) fn default_map(place: &Place, target: &Target) -> Vec<(String, String)> {
    let mut map = Vec::new();
    if !target.home.is_empty() {
        map.push((place.home.root().display().to_string(), target.home.clone()));
    }
    if !target.user_home.is_empty() {
        map.push((place.user_home.display().to_string(), target.user_home.clone()));
    }
    map.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
    map
}

/// The folder under the target's home where project folders from outside
/// this computer's home folders go: named for this computer, with no spaces,
/// since commands that name those paths are rewritten to the new ones.
pub(crate) fn moved_from_dir(machine: &str) -> String {
    let mut slug = String::new();
    for c in machine.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if c == '\'' || c == '’' {
            // "Matt's Mac" reads as matts-mac.
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() { "moved-from-another-computer".to_owned() } else { format!("moved-from-{slug}") }
}

/// Where each of `folders` outside this computer's home folders goes on the
/// target, so a move never stops at one: under the target user's home, in
/// `moved_from_dir`, keeping its whole path (/Volumes/Data/Assets becomes
/// ~/moved-from-mac/Volumes/Data/Assets). One inside another goes with it.
/// Empty when the target doesn't say where its home is.
pub(crate) fn outside_map(place: &Place, target: &Target, folders: impl IntoIterator<Item = PathBuf>) -> Vec<(String, String)> {
    let base = if target.user_home.is_empty() { &target.home } else { &target.user_home };
    if base.is_empty() {
        return Vec::new();
    }
    let base = Path::new(base).join(moved_from_dir(crate::remote::machine_name()));
    let mut outside: Vec<PathBuf> = folders
        .into_iter()
        .filter(|folder| folder.is_absolute() && folder.parent().is_some())
        .filter(|folder| !folder.starts_with(place.home.root()) && !folder.starts_with(&place.user_home))
        .collect();
    outside.sort();
    outside.dedup();
    let mut map: Vec<(String, String)> = Vec::new();
    for folder in outside {
        if map.iter().any(|(from, _)| folder.starts_with(from)) {
            continue;
        }
        let rest: PathBuf = folder.components().filter(|part| matches!(part, std::path::Component::Normal(_))).collect();
        map.push((folder.display().to_string(), base.join(rest).display().to_string()));
    }
    map
}

/// The paths the move gives the target: `default_map`, then each of the
/// tree's folders outside the home folders (`outside_map`).
pub(crate) fn plan_map(place: &Place, target: &Target, tree: &Tree) -> Vec<(String, String)> {
    let mut map = default_map(place, target);
    map.extend(outside_map(place, target, tree.manifest.folders.iter().map(|folder| PathBuf::from(&folder.path))));
    map
}

/// The commands the move's folders and agents' tasks need on the target.
pub(crate) fn tools(tree: &Tree) -> Vec<String> {
    let mut tools = BTreeSet::new();
    if tree.manifest.folders.iter().any(|folder| folder.kind == "git") || !tree.manifest.checkouts.is_empty() {
        tools.insert("git".to_owned());
    }
    for task in &tree.manifest.tasks {
        tools.extend(commands(&task.command));
    }
    tools.into_iter().collect()
}

/// The shell's own words, never commands to look for on another computer
/// (friend bug #25): builtins and keywords.
const SHELL_WORDS: &str = "! . : [ [[ alias bg bind break builtin caller case cd command compgen complete continue coproc declare dirs disown do done echo elif else enable esac eval exec exit export false fg fi for function getopts hash help history if in jobs kill let local logout mapfile popd printf pushd pwd read readarray readonly return select set shift shopt source suspend test then time times trap true type typeset ulimit umask unalias unset until wait while";

/// Words that run the command after them: it's that one that's needed.
const RUNS_NEXT: &str = "! { } builtin command do elif else env exec if nohup then time until while";

/// The commands a shell command line runs: each simple command's first
/// word, past assignments, redirections and words such as exec, leaving
/// out the shell's own words. Quoting isn't followed: a word that isn't a
/// plain command name is passed over.
fn commands(line: &str) -> Vec<String> {
    // Redirections such as 2>&1 and &> aren't separators.
    let line = line.replace(">&", "> ").replace("<&", "< ").replace("&>", " >");
    let mut found = Vec::new();
    for part in line.split([';', '&', '|', '\n', '(', ')', '`']) {
        let mut words = part.split_whitespace();
        let mut redirected = false;
        let word = words.find(|word| {
            if std::mem::take(&mut redirected) {
                return false;
            }
            let redirection = word.trim_start_matches(|c: char| c.is_ascii_digit());
            if redirection.starts_with(['<', '>']) {
                // `> file` names its file in the next word.
                redirected = redirection.trim_start_matches(['<', '>']).is_empty();
                return false;
            }
            !word.contains('=') && !RUNS_NEXT.split(' ').any(|next| next == *word) && !word.starts_with('-')
        });
        let name = word.unwrap_or_default().rsplit('/').next().unwrap_or_default();
        if name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.+".contains(c))
            && !SHELL_WORDS.split(' ').any(|shell| shell == name)
        {
            found.push(name.to_owned());
        }
    }
    found
}

/// Plans and stages the tree afresh into `<home>/moving/staging`.
/// Its manifest is final every round: this copy is fenced at the commit.
pub(crate) async fn prepare(place: &Place, options: &Options, export_id: &str) -> Result<Tree, String> {
    let (place, options, export_id) = (place.clone(), options.clone(), export_id.to_owned());
    tokio::task::spawn_blocking(move || {
        let plan = move_export::plan(&place, &options).map_err(|refused| refused.reason)?;
        let staging = moving_dir(&place.home).join("staging");
        let _ = std::fs::remove_dir_all(&staging);
        crate::remote::private_dir(&staging).map_err(|error| format!("{}: {error}", staging.display()))?;
        move_export::stage(&place, &export_id, true, &plan, &staging)
    })
    .await
    .map_err(|error| error.to_string())?
}

// ---- `factoryd move` ----

/// What `factoryd move` says: only the person starts a move, in the app.
const PERSONS: &str = "Only you can start a move: open Claktory on this computer or a device connected to it, and choose Move to another computer… (Settings → General).";

/// `factoryd [--dev] move …`: refused, whatever it's given
/// (server-authorize.md, Moves 2). A move hands over everything, and a
/// terminal can't tell the person from an agent's command, which can make
/// itself one, so only the person starts a move, from Claktory's app. Exit
/// 3 (`cli::Exit::Refused`); with --json, one line whose error is
/// `person_only`, as the commands the tool token can't do say.
pub fn main(_dev: bool, args: &[String]) -> ExitCode {
    if args.iter().any(|arg| arg == "--json") {
        println!("{}", serde_json::json!({"ok": false, "error": "person_only", "message": PERSONS}));
    } else {
        eprintln!("{PERSONS}");
    }
    ExitCode::from(3)
}

/// What drives a move talks to: the person, wherever they are.
pub(crate) trait Person: Send {
    /// Something to tell them.
    fn say(&mut self, text: &str);
    /// Whether they see `digits` on `machine`. The tests' whole move only.
    #[cfg(test)]
    fn digits_match(&mut self, _machine: &str, _digits: &str) -> bool {
        false
    }
    /// What the target said of the plan, which carries `plan_bytes`.
    fn checked(&mut self, _checked: &Checked, _plan_bytes: u64) {}
    /// Round `n` starts.
    fn round_started(&mut self, _n: u32) {}
    /// Round `n` is done.
    fn round_done(&mut self, _n: u32, _rounded: &Rounded) {}
    /// The target's import, a step at a time.
    fn importing(&mut self, _step: &str, text: &str) {
        if !text.is_empty() {
            self.say(&format!("  {text}"));
        }
    }
    /// They asked to freeze now rather than once the copy settles.
    fn freeze_now(&self) -> bool {
        false
    }
    /// Within a round: what the target has of it, and its size.
    fn sending(&mut self, _have: u64, _total: u64) {}
    /// The connection to `machine` dropped: it's being dialed again.
    fn redialing(&mut self, machine: &str) {
        self.say(&format!("  Lost the connection to {machine}; trying to reach it again…"));
    }
    /// It's back.
    fn redialed(&mut self, _machine: &str) {}
}

/// The whole move: pairing, pre-copy, the freeze, the last round, the
/// commit. What happened, in a sentence, either way. The tests' (`factoryd
/// move` drove it before #546; the app's is `move_run`).
#[cfg(test)]
pub(crate) async fn drive(place: &Place, options: &Options, code: ReceiveCode, person: &mut dyn Person) -> Result<String, String> {
    if !place.home.config_path().is_file() {
        return Err(format!("There's no Claktory at {}.", place.home.root().display()));
    }
    if move_export::read_moved(&place.home).is_some() {
        return Err("This Claktory already moved to another computer. To bring it back here, run factoryd export --restore.".to_owned());
    }
    if crate::move_run::saved(&place.home).is_some() {
        return Err("A move is already under way from the app. Follow it there, or call it off there first.".to_owned());
    }
    move_export::stoppable(place, "factoryd move").map_err(|refused| refused.reason)?;
    let mut mover = Mover::new(&place.home, code).await?;
    let machine = mover.code.machine.clone();
    person.say(&format!("Connecting to {machine}…"));
    let mut link = mover.connect().await.map_err(|problem| match problem {
        Problem::Lost(detail) => format!("Can't reach {machine} ({detail}). Check that factoryd receive still runs there, and try again."),
        problem => problem.text(&machine),
    })?;
    let machine = link.target.machine.clone();
    // A move from here starts fresh (one under way is refused above), so the
    // person always confirms the digits: a target claiming to know this move
    // never skips that.
    if link.target.resumed {
        let why = format!("{machine} says it knows a move this computer never started, so nothing was moved.");
        link.fail(&why).await;
        forget(&place.home);
        return Err(why);
    }
    {
        let target = &link.target;
        person.say(&format!("Moving to {machine} ({} {}, Claktory {}).", target.os, target.arch, target.version));
        let digits = link.target.digits.clone();
        if !tokio::task::block_in_place(|| person.digits_match(&machine, &digits)) {
            let source = crate::remote::machine_name();
            link.fail(&format!("The digits weren't confirmed on {source}, so nothing was moved.")).await;
            forget(&place.home);
            return Err("Nothing was moved.".to_owned());
        }
        person.say(&format!("Now confirm the same digits on {machine}…"));
        if let Err(problem) = link.confirm().await {
            forget(&place.home);
            return Err(format!("{} Nothing was moved.", problem.text(&machine)));
        }
    }
    let export_id = move_export::new_export_id();
    let copying = async {
        tokio::select! {
            copied = copy(place, options, &mut mover, &mut link, &export_id, 1, person) => copied,
            _ = tokio::signal::ctrl_c() => Err(Problem::Said("You stopped the move.".to_owned())),
        }
    };
    let copied = match copying.await {
        Ok(copied) => copied,
        Err(problem) => {
            let why = problem.text(&machine);
            link.cancel(&why).await;
            forget(&place.home);
            return Err(format!("{why} Nothing was moved."));
        }
    };
    // Frozen: from here a failure starts Claktory again.
    person.say("Stopping Claktory for the last round…");
    if let Err(refused) = move_export::stop(place) {
        link.cancel(&refused.reason).await;
        forget(&place.home);
        return Err(refused.reason);
    }
    crate::uninstall::stop_leftovers(&place.home);
    let unfreeze = |why: String| {
        let started = move_export::start(place);
        forget(&place.home);
        match started {
            Ok(()) => format!("{why} Nothing was moved, and Claktory runs here again, as it was."),
            Err(problem) => {
                format!("{why} Nothing was moved, but Claktory didn't start again by itself ({problem}): start it from the Claktory app.")
            }
        }
    };
    let n = copied.round + 1;
    let finishing = async {
        tokio::select! {
            finished = finish(place, options, &mut mover, &mut link, &export_id, n, person) => finished,
            _ = tokio::signal::ctrl_c() => Err(Problem::Said("You stopped the move.".to_owned())),
        }
    };
    if let Err(problem) = finishing.await {
        let why = problem.text(&machine);
        link.cancel(&why).await;
        return Err(unfreeze(why));
    }
    // Fenced: from here the target is the Claktory, once it says done.
    let moved = move_export::Moved {
        export_id: export_id.clone(),
        epoch: copied.epoch,
        machine: crate::remote::machine_name().to_owned(),
        to: machine.clone(),
        to_os: link.target.os.clone(),
        ..move_export::Moved::default()
    };
    if let Err(refused) = move_export::tombstone(place, moved) {
        link.cancel(&refused.reason).await;
        return Err(unfreeze(refused.reason));
    }
    let at_ms = crate::transcript::now_ms();
    let committed = commit(&mover, &mut link, at_ms, &export_id, person).await;
    mover.close().await;
    match committed {
        Ok(()) => {}
        Err(why) => {
            {
                // It refused the commit, so it never started: this copy comes back.
                let back = move_export::restore(place).map_err(|refused| refused.reason);
                forget(&place.home);
                return Err(match back {
                    Ok(_) => format!("{why} Nothing was moved, and Claktory runs here again, as it was."),
                    Err(problem) => {
                        format!(
                            "{why} Nothing was moved, but this copy couldn't come back by itself ({problem}): run factoryd export --restore."
                        )
                    }
                });
            }
        }
    }
    forget(&place.home);
    Ok(format!(
        "Moved to {machine}. Your devices find it there. This copy is kept, stopped, until you delete it: once the moved Claktory works, run factoryd export --delete-moved."
    ))
}

/// Fenced: says `commit` until the target says done, dialing again for as
/// long as it takes. Err with the target's sentence when it refuses.
pub(crate) async fn commit(mover: &Mover, link: &mut Link, at_ms: i64, export_id: &str, person: &mut dyn Person) -> Result<(), String> {
    let machine = link.target.machine.clone();
    let (mut told, mut broken) = (false, false);
    loop {
        if broken {
            match mover.redial(REDIAL).await {
                Ok(again) => *link = again,
                Err(problem) => {
                    tracing::info!(problem = %problem.text(&machine), "the move's target can't be reached to commit; trying again");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            }
        }
        match link.commit(at_ms, export_id).await {
            Ok(()) => return Ok(()),
            Err(Problem::Said(why)) => return Err(why),
            // commit skips `ready`; never said here.
            Err(Problem::Ready) => {}
            Err(Problem::Lost(detail)) => {
                if !told {
                    person.say(&format!(
                        "Lost {machine} at the last step ({detail}); trying again. This copy has stopped for good: if {machine} can't be reached again, run factoryd receive --committed there."
                    ));
                    told = true;
                }
                broken = true;
            }
        }
    }
}

/// The move is over, either way: its key and staging go.
pub(crate) fn forget(home: &Home) {
    let _ = std::fs::remove_dir_all(moving_dir(home));
}

/// What pre-copy leaves for the last round.
pub(crate) struct Copied {
    /// The last round's number.
    pub(crate) round: u32,
    pub(crate) epoch: u64,
}

/// One round, dialing again if the connection drops (and saying the plan
/// again on the new one).
#[allow(clippy::too_many_arguments)]
async fn round(
    place: &Place,
    mover: &mut Mover,
    link: &mut Link,
    n: u32,
    last: bool,
    tree: &Tree,
    person: &mut dyn Person,
) -> Result<Rounded, Problem> {
    let listing = mover.list(tree).await.map_err(Problem::Said)?;
    let machine = link.target.machine.clone();
    loop {
        match link.round(mover, n, last, &listing, &mut |have, total| person.sending(have, total)).await {
            Err(Problem::Lost(detail)) => {
                tracing::info!(%detail, "the move's connection dropped; dialing again");
                person.redialing(&machine);
                *link = mover.redial(REDIAL).await.map_err(|problem| unanswered(&machine, problem))?;
                person.redialed(&machine);
                let map = plan_map(place, &link.target, tree);
                link.plan(tree, &map, &tools(tree)).await?;
            }
            done => return done,
        }
    }
}

/// Pre-copy: the plan, then rounds while Claktory runs, until they settle.
/// Rounds are numbered from `first` (1, or where a restarted move was).
pub(crate) async fn copy(
    place: &Place,
    options: &Options,
    mover: &mut Mover,
    link: &mut Link,
    export_id: &str,
    first: u32,
    person: &mut dyn Person,
) -> Result<Copied, Problem> {
    let machine = link.target.machine.clone();
    let mut n = first;
    loop {
        let started = Instant::now();
        person.round_started(n);
        let tree = prepare(place, options, export_id).await.map_err(Problem::Said)?;
        if n == first {
            let checked = link.plan(&tree, &plan_map(place, &link.target, &tree), &tools(&tree)).await?;
            if !checked.problems.is_empty() {
                return Err(Problem::Said(format!("{machine} can't take this Claktory: {}", checked.problems.join(" "))));
            }
            person.checked(&checked, tree.manifest.bytes);
            person.say(&format!("Copying {} to {machine} while Claktory keeps working…", size_text(tree.manifest.bytes)));
            for (from, to) in checked.map.iter().filter(|(from, to)| from != to) {
                person.say(&format!("  {from} goes to {to}"));
            }
            if !checked.missing.is_empty() {
                person
                    .say(&format!("  {machine} doesn't have {}: install them there for agents that use them.", checked.missing.join(", ")));
            }
        }
        let rounded = round(place, mover, link, n, false, &tree, person).await?;
        person.say(&format!(
            "  Round {n}: sent {}; {machine} has {} of {}.",
            size_text(rounded.sent),
            size_text(rounded.have),
            size_text(rounded.total)
        ));
        person.round_done(n, &rounded);
        let settled = rounded.sent < SETTLED_BYTES || started.elapsed() < SETTLED_TIME || n + 1 - first >= MAX_ROUNDS;
        if settled || person.freeze_now() {
            return Ok(Copied { round: n, epoch: tree.manifest.epoch });
        }
        n += 1;
    }
}

/// Frozen: the last round, then import on the target until it's ready.
pub(crate) async fn finish(
    place: &Place,
    options: &Options,
    mover: &mut Mover,
    link: &mut Link,
    export_id: &str,
    n: u32,
    person: &mut dyn Person,
) -> Result<(), Problem> {
    let machine = link.target.machine.clone();
    let tree = prepare(place, options, export_id).await.map_err(Problem::Said)?;
    let rounded = match round(place, mover, link, n, true, &tree, person).await {
        Err(Problem::Ready) => return Ok(()),
        rounded => rounded?,
    };
    person.say(&format!("  Last round: sent {}. {machine} is setting it up…", size_text(rounded.sent)));
    loop {
        match link.ready(&mut |step: &str, text: &str| person.importing(step, text)).await {
            // The target carries on importing without us, and turns other
            // connections away meanwhile: dialed again, it says ready (or
            // why not) when it's done.
            Err(Problem::Lost(detail)) => {
                tracing::info!(%detail, "the move's connection dropped during import; dialing again");
                person.redialing(&machine);
                *link = mover.redial_heard(PATIENCE, UNHEARD).await.map_err(|problem| unanswered(&machine, problem))?;
                person.redialed(&machine);
            }
            done => return done,
        }
    }
}

// Moving isn't on Windows yet (`move_run::NOT_ON_WINDOWS`).
#[cfg(all(test, unix))]
mod tests {
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::move_bundle::Manifest;
    use crate::move_export::Service;

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("move-source-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp");
        dir
    }

    /// A small Claktory, stopped, with a database.
    fn claktory() -> Place {
        let user_home = temp();
        let home = user_home.join(".factory");
        std::fs::create_dir_all(home.join("agents/ana")).unwrap();
        std::fs::write(home.join("config.json"), r#"{"projects":[]}"#).unwrap();
        std::fs::write(home.join("agents/ana/segment-1.jsonl"), "hello\n".repeat(50_000)).unwrap();
        let db = rusqlite::Connection::open(home.join("factory.db")).unwrap();
        db.execute_batch("CREATE TABLE notes (text TEXT); INSERT INTO notes VALUES ('kept');").unwrap();
        drop(db);
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        Place { home: Home::new(home), port, user_home, dev: true, service: Service::None }
    }

    /// How the fake target behaves.
    #[derive(Clone, Default)]
    struct Script {
        problems: Vec<String>,
        /// Fails after the last round instead of being ready.
        import_fails: Option<String>,
        /// Drops the connection once, when this round starts.
        drop_at_round: Option<u32>,
        /// Says it knows the move (resumed) on the first connection too.
        claims_resumed: bool,
        /// Stops for good once the last round is in, as a receive killed
        /// there would (friend bug #22).
        dies_at_import: bool,
    }

    /// What the fake target saw.
    #[derive(Default)]
    struct Seen {
        /// The tree as staged: each path's bytes (files) or None.
        tree: HashMap<String, Option<Vec<u8>>>,
        /// Paths whose blocks it needed, by round.
        needed: Vec<(u32, bool, Vec<String>)>,
        connections: u32,
        confirmed: bool,
        plans: u32,
        committed: Option<String>,
        cancelled: Option<String>,
        failed: Option<String>,
        dead: bool,
    }

    /// The target's side, just enough of it: one key may claim the code.
    async fn target(script: Script) -> (ReceiveCode, Arc<Mutex<Seen>>) {
        let endpoint = Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .alpns(vec![move_wire::ALPN.to_vec()])
            .clear_ip_transports()
            .bind_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .unwrap()
            .bind()
            .await
            .unwrap();
        let address = endpoint.bound_sockets().into_iter().find(SocketAddr::is_ipv4).unwrap();
        let code = ReceiveCode {
            endpoint: endpoint.id().to_string(),
            secret: "5e".repeat(32),
            machine: "cloud-box".to_owned(),
            version: crate::update::VERSION.to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            relays: Vec::new(),
            addresses: vec![address],
        };
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (secret, shared) = (code.secret.clone(), seen.clone());
        tokio::spawn(async move {
            let mut script = script;
            let mut claimed: Option<String> = None;
            while let Some(incoming) = endpoint.accept().await {
                let Ok(connection) = incoming.await else { continue };
                let source = connection.remote_id().to_string();
                let Ok((mut send, mut recv)) = connection.accept_bi().await else { continue };
                shared.lock().unwrap().connections += 1;
                let ended = serve(&mut send, &mut recv, &secret, &source, &mut claimed, &mut script, &shared).await;
                if ended {
                    let _ = send.finish();
                    let _ = tokio::time::timeout(Duration::from_secs(2), connection.closed()).await;
                }
                if shared.lock().unwrap().dead {
                    // What it wrote arrives first, then it's gone.
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    connection.close(0u32.into(), b"gone");
                    endpoint.close().await;
                    break;
                }
            }
        });
        (code, seen)
    }

    /// One connection; true when it ended in words (not dropped).
    async fn serve(
        send: &mut SendStream,
        recv: &mut RecvStream,
        secret: &str,
        source: &str,
        claimed: &mut Option<String>,
        script: &mut Script,
        seen: &Arc<Mutex<Seen>>,
    ) -> bool {
        let Ok((Frame::Hello { secret: given, .. }, _)) = move_wire::read(recv).await else { return false };
        if given != secret || claimed.as_ref().is_some_and(|key| key != source) {
            let _ = move_wire::write(send, &Frame::Failed { why: "Not this code.".into() }, &[]).await;
            return true;
        }
        let resumed = claimed.is_some() || script.claims_resumed;
        let welcome = Frame::Welcome {
            version: crate::update::VERSION.into(),
            machine: "cloud-box".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            home: "/home/matt/.factory".into(),
            user_home: "/home/matt".into(),
            free_bytes: 1 << 40,
            local_disk: true,
            fresh: true,
            resumed,
        };
        move_wire::write(send, &welcome, &[]).await.unwrap();
        loop {
            let Ok((frame, _)) = move_wire::read(recv).await else { return false };
            match frame {
                Frame::Confirmed {} => {
                    *claimed = Some(source.to_owned());
                    seen.lock().unwrap().confirmed = true;
                    move_wire::write(send, &Frame::Confirmed {}, &[]).await.unwrap();
                }
                Frame::Plan { map, .. } => {
                    seen.lock().unwrap().plans += 1;
                    let checked =
                        Frame::Checked { problems: script.problems.clone(), map, missing: vec!["flutter".into()], free_bytes: 1 << 40 };
                    move_wire::write(send, &checked, &[]).await.unwrap();
                }
                Frame::Round { n, last } => {
                    if script.drop_at_round == Some(n) {
                        script.drop_at_round = None;
                        return false;
                    }
                    if !round(send, recv, n, last, seen).await {
                        return false;
                    }
                    if last && script.dies_at_import {
                        seen.lock().unwrap().dead = true;
                        return false;
                    }
                    if last {
                        move_wire::write(send, &Frame::Progress { step: "verify".into(), text: "Checking it all".into() }, &[])
                            .await
                            .unwrap();
                        let after = match &script.import_fails {
                            Some(why) => Frame::Failed { why: why.clone() },
                            None => Frame::Ready { old_app_code: None },
                        };
                        move_wire::write(send, &after, &[]).await.unwrap();
                    }
                }
                Frame::Commit { export_id, .. } => {
                    seen.lock().unwrap().committed = Some(export_id);
                    move_wire::write(send, &Frame::Done {}, &[]).await.unwrap();
                    return true;
                }
                Frame::Cancel { why } => {
                    seen.lock().unwrap().cancelled = Some(why);
                    return true;
                }
                Frame::Failed { why } => {
                    seen.lock().unwrap().failed = Some(why);
                    return true;
                }
                _ => return false,
            }
        }
    }

    /// A round: the listing, the blocks it lacks, each checked.
    async fn round(send: &mut SendStream, recv: &mut RecvStream, n: u32, last: bool, seen: &Arc<Mutex<Seen>>) -> bool {
        let mut entries = Vec::new();
        loop {
            match move_wire::read(recv).await {
                Ok((Frame::Entry(entry), _)) => entries.push(entry),
                Ok((Frame::RoundEnd { n: end }, _)) if end == n => break,
                _ => return false,
            }
        }
        let mut needed = Vec::new();
        for entry in entries.iter().filter(|entry| entry.kind == Kind::File) {
            let have = seen.lock().unwrap().tree.get(&entry.path).cloned().flatten().unwrap_or_default();
            let blocks: Vec<u32> = (0..entry.hashes.len() as u32)
                .filter(|index| {
                    let start = (*index as usize) * entry.block as usize;
                    let end = (start + entry.block as usize).min(have.len());
                    start >= have.len() || move_wire::block_hash(&have[start..end]) != entry.hashes[*index as usize]
                })
                .collect();
            if !blocks.is_empty() {
                needed.push(entry.path.clone());
                move_wire::write(send, &Frame::Need { path: entry.path.clone(), blocks }, &[]).await.unwrap();
            }
        }
        move_wire::write(send, &Frame::NeedEnd { n }, &[]).await.unwrap();
        let mut tree: HashMap<String, Option<Vec<u8>>> = seen.lock().unwrap().tree.clone();
        loop {
            match move_wire::read(recv).await {
                Ok((Frame::Block { path, index, .. }, bytes)) => {
                    let entry = entries.iter().find(|entry| entry.path == path).unwrap();
                    assert_eq!(move_wire::block_hash(&bytes), entry.hashes[index as usize], "{path} block {index}");
                    let file = tree.entry(path).or_insert(Some(Vec::new())).get_or_insert_with(Vec::new);
                    let start = index as usize * entry.block as usize;
                    file.resize(file.len().max(start + bytes.len()), 0);
                    file[start..start + bytes.len()].copy_from_slice(&bytes);
                }
                Ok((Frame::BlocksEnd { n: end }, _)) if end == n => break,
                _ => return false,
            }
        }
        // The listing is the tree: what isn't listed goes, files are cut to size.
        tree.retain(|path, _| entries.iter().any(|entry| entry.path == *path));
        for entry in &entries {
            match entry.kind {
                Kind::File => {
                    if let Some(Some(file)) = tree.get_mut(&entry.path) {
                        file.truncate(entry.size as usize);
                    } else {
                        tree.insert(entry.path.clone(), Some(Vec::new()));
                    }
                }
                _ => {
                    tree.insert(entry.path.clone(), None);
                }
            }
        }
        let total: u64 = entries.iter().map(|entry| entry.size).sum();
        {
            let mut seen = seen.lock().unwrap();
            seen.tree = tree;
            seen.needed.push((n, last, needed));
        }
        let done = Frame::RoundDone { n, bytes_have: total, bytes_total: total, stale: 0 };
        move_wire::write(send, &done, &[]).await.unwrap();
        true
    }

    struct Auto {
        yes: bool,
        said: Vec<String>,
    }

    impl Person for Auto {
        fn say(&mut self, text: &str) {
            self.said.push(text.to_owned());
        }

        fn digits_match(&mut self, _machine: &str, digits: &str) -> bool {
            assert_eq!(digits.len(), 7);
            self.yes
        }
    }

    fn file(seen: &Seen, path: &str) -> Vec<u8> {
        seen.tree.get(path).cloned().flatten().unwrap_or_else(|| panic!("{path} wasn't moved"))
    }

    #[test]
    fn folders_outside_the_home_folders_go_under_the_targets_home() {
        assert_eq!(moved_from_dir("Matt’s MacBook Pro"), "moved-from-matts-macbook-pro");
        assert_eq!(moved_from_dir("build-01.example.com"), "moved-from-build-01-example-com");
        assert_eq!(moved_from_dir("…"), "moved-from-another-computer");
        let place = Place {
            home: Home::new(PathBuf::from("/Users/me/.factory")),
            port: 0,
            user_home: PathBuf::from("/Users/me"),
            dev: true,
            service: Service::None,
        };
        let target = Target { home: "/home/you/.factory".to_owned(), user_home: "/home/you".to_owned(), ..Target::default() };
        let folders = [
            "/Users/me/web",
            "/Users/me/.factory/workspace/site",
            "/Volumes/Data/Assets/sub",
            "/Volumes/Data/Assets",
            "/opt/site",
            "/",
            "relative",
        ];
        let map = outside_map(&place, &target, folders.map(PathBuf::from));
        let under = format!("/home/you/{}", moved_from_dir(crate::remote::machine_name()));
        assert_eq!(
            map,
            [
                ("/Volumes/Data/Assets".to_owned(), format!("{under}/Volumes/Data/Assets")),
                ("/opt/site".to_owned(), format!("{under}/opt/site")),
            ]
        );
        // A target that doesn't say where its home is gets none.
        assert!(outside_map(&place, &Target::default(), folders.map(PathBuf::from)).is_empty());
    }

    #[test]
    fn the_commands_a_task_needs_leave_out_the_shells_own_words() {
        let cases: [(&str, &[&str]); 9] = [
            ("cd app && exec flutter run -d macos", &["flutter"]),
            ("cd /tmp; exec ./serve.sh --port 80", &["serve.sh"]),
            ("RUST_LOG=debug cargo test 2>&1 | tee log", &["cargo", "tee"]),
            ("export A=1; source .env && npm start >&2", &["npm"]),
            ("for f in *.md; do pandoc \"$f\"; done", &["pandoc"]),
            ("if [ -e x ]; then make; else true; fi", &["make"]),
            ("{ tail -f log & t=$!; wait; } | ssh box 'cat > x'", &["tail", "ssh"]),
            ("env FOO=1 nohup /usr/local/bin/node app.js > out.log 2>&1 &", &["node"]),
            ("echo \"$(date +%s)\" > \"${HOME}/stamp\"", &["date"]),
        ];
        for (line, expected) in cases {
            assert_eq!(commands(line), expected, "{line}");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_claktory_moves_and_this_copy_is_fenced() {
        let place = claktory();
        let (code, seen) = target(Script::default()).await;
        let mut person = Auto { yes: true, said: Vec::new() };
        let done = drive(&place, &Options::default(), code, &mut person).await.expect("moved");
        assert!(done.starts_with("Moved to cloud-box."), "{done}");
        let seen = seen.lock().unwrap();
        assert!(seen.confirmed);
        // The last round's manifest is the move's, final, and committed.
        let manifest: Manifest = serde_json::from_slice(&file(&seen, "manifest.json")).unwrap();
        assert!(manifest.is_final);
        assert_eq!(seen.committed.as_deref(), Some(manifest.export_id.as_str()));
        assert_eq!(file(&seen, "home/config.json"), br#"{"projects":[]}"#);
        assert_eq!(file(&seen, "home/agents/ana/segment-1.jsonl"), "hello\n".repeat(50_000).into_bytes());
        // The database went as a snapshot that opens.
        let copy = temp().join("factory.db");
        std::fs::write(&copy, file(&seen, "home/factory.db")).unwrap();
        let text: String = rusqlite::Connection::open(&copy).unwrap().query_row("SELECT text FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(text, "kept");
        // The last round sent only what changed: never the unchanged transcript.
        let (_, last, needed) = seen.needed.last().unwrap();
        assert!(last);
        assert!(!needed.contains(&"home/agents/ana/segment-1.jsonl".to_owned()), "{needed:?}");
        assert!(!seen.tree.keys().any(|path| path.starts_with("home/moving")), "the move's own key never goes");
        // This copy: fenced, saying where it went, the move's key gone.
        let moved = move_export::read_moved(&place.home).expect("moved.json");
        assert_eq!(
            (moved.to.as_str(), moved.to_os.as_str(), moved.export_id.as_str()),
            ("cloud-box", "linux", manifest.export_id.as_str())
        );
        assert!(!moving_dir(&place.home).exists());
        assert!(person.said.iter().any(|said| said.contains("doesn't have flutter")), "{:?}", person.said);
    }

    /// A person who must never be asked.
    struct Absent;

    impl Person for Absent {
        fn say(&mut self, _text: &str) {}

        fn digits_match(&mut self, _machine: &str, _digits: &str) -> bool {
            panic!("asked");
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_target_claiming_a_move_never_confirmed_here_gets_nothing() {
        let place = claktory();
        std::fs::create_dir_all(place.home.root().join("vault")).unwrap();
        let (code, seen) = target(Script { claims_resumed: true, ..Script::default() }).await;
        let error = drive(&place, &Options::default(), code, &mut Absent).await.unwrap_err();
        assert!(error.contains("never started"), "{error}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let seen = seen.lock().unwrap();
        assert!(seen.tree.is_empty() && seen.plans == 0 && seen.committed.is_none(), "nothing went");
        assert!(move_export::read_moved(&place.home).is_none());
        assert!(place.home.root().join("vault").is_dir(), "nothing was fenced");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_target_that_cant_take_it_moves_nothing() {
        let place = claktory();
        let (code, seen) = target(Script { problems: vec!["There isn't enough free space here.".into()], ..Script::default() }).await;
        let error = drive(&place, &Options::default(), code, &mut Auto { yes: true, said: Vec::new() }).await.unwrap_err();
        assert!(error.contains("There isn't enough free space here.") && error.ends_with("Nothing was moved."), "{error}");
        assert!(move_export::read_moved(&place.home).is_none());
        assert!(!moving_dir(&place.home).exists());
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(seen.lock().unwrap().cancelled.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn digits_not_confirmed_here_tell_the_target_and_move_nothing() {
        let place = claktory();
        let (code, seen) = target(Script::default()).await;
        let error = drive(&place, &Options::default(), code, &mut Auto { yes: false, said: Vec::new() }).await.unwrap_err();
        assert_eq!(error, "Nothing was moved.");
        tokio::time::sleep(Duration::from_millis(300)).await;
        let seen = seen.lock().unwrap();
        assert!(!seen.confirmed && seen.failed.as_deref().is_some_and(|why| why.contains("digits weren't confirmed")));
        assert!(!moving_dir(&place.home).exists());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_import_leaves_this_copy_running_as_it_was() {
        let place = claktory();
        std::fs::create_dir_all(place.home.root().join("vault")).unwrap();
        let (code, _) = target(Script { import_fails: Some("Import stopped: the disk is full.".into()), ..Script::default() }).await;
        let error = drive(&place, &Options::default(), code, &mut Auto { yes: true, said: Vec::new() }).await.unwrap_err();
        assert!(error.starts_with("Import stopped: the disk is full. Nothing was moved, and Claktory runs here again"), "{error}");
        assert!(move_export::read_moved(&place.home).is_none());
        assert!(place.home.root().join("vault").is_dir(), "nothing was fenced");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dropped_connection_dials_again_and_carries_on() {
        let place = claktory();
        let (code, seen) = target(Script { drop_at_round: Some(1), ..Script::default() }).await;
        drive(&place, &Options::default(), code, &mut Auto { yes: true, said: Vec::new() }).await.expect("moved");
        let seen = seen.lock().unwrap();
        assert!(seen.connections >= 2 && seen.plans >= 2, "dialed again and said the plan again");
        assert!(seen.committed.is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_target_that_stops_while_importing_is_given_up_on() {
        let place = claktory();
        std::fs::create_dir_all(place.home.root().join("vault")).unwrap();
        let (code, seen) = target(Script { dies_at_import: true, ..Script::default() }).await;
        let started = Instant::now();
        let error = drive(&place, &Options::default(), code, &mut Auto { yes: true, said: Vec::new() }).await.unwrap_err();
        assert!(
            error.starts_with("cloud-box stopped answering, so factoryd receive has likely stopped there.")
                && error.contains("Nothing was moved, and Claktory runs here again"),
            "{error}"
        );
        assert!(started.elapsed() < PATIENCE / 2, "gave up while nothing answered, not after import's patience");
        let seen = seen.lock().unwrap();
        assert!(seen.dead && seen.committed.is_none());
        drop(seen);
        assert!(move_export::read_moved(&place.home).is_none());
        assert!(place.home.root().join("vault").is_dir(), "nothing was fenced");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_older_target_is_refused_before_dialing() {
        let place = claktory();
        let code = ReceiveCode { version: "v0.0.1".into(), machine: "old-box".into(), ..ReceiveCode::default() };
        let error = Mover::new(&place.home, code).await.err().unwrap();
        assert!(error.starts_with("old-box has Claktory v0.0.1, older than this one"), "{error}");
    }
}
