//! `factoryd receive`: the target's side of moving a Claktory over the
//! network (#217, phase 2; docs/MOVE.md, "The handshake"). It shows a
//! receive code, lets one source in once both people have confirmed the
//! same six digits, answers its checks, takes its rounds of blocks into
//! `<home>/import/bundle` (a bundle's tree, unpacked) and, after the final
//! round, runs the import up to Verify. It says `ready` then, and finishes
//! only once the source has committed (`done`): until then
//! `<home>/import/state.json` keeps this home's backend from starting.
//!
//! - Its endpoint has a key of its own (`<home>/import/receive/key`), never
//!   the Claktory's, with the code's secret beside it: running receive again
//!   shows the same code, and carries on.
//! - What it has stays across dropped connections and restarts: staged
//!   files, and an index of their blocks' hashes (`receive/index.json`),
//!   checked against each file's size and time when it starts.
//! - A failure before the home is switched over keeps what's staged, and
//!   waits for the source to try again; one after it undoes the import.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::unix::fs::{FileExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, RelayMode, SecretKey};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{
    FORMAT, ImportError, Manifest, PathMap, State, Step, dir, failed, finish, fresh, load_state, local_disk, lock, not_running, on_path,
    path_map, platform, problems, read_manifest, report, running, running_here, save_state, steps, too_new, undo,
};
use crate::locking::lock as locked;
use crate::encoding::{hex, unhex};
use crate::home::Home;
use crate::move_wire::{self, Entry, Frame, Kind, ReceiveCode};

/// How long a source has to say hello once it's connected.
const HELLO_WITHIN: Duration = Duration::from_secs(30);
/// How long making the code waits for a relay, so the code can name it.
const ONLINE_WAIT: Duration = Duration::from_secs(5);
/// How long a wrong secret waits for its answer.
const WRONG_PAUSE: Duration = Duration::from_secs(1);
/// How long the connection stays for the source to read the last frame.
const LINGER: Duration = Duration::from_secs(5);
/// After `done`: how long it waits for the source to hang up.
const DONE_LINGER: Duration = Duration::from_secs(60);
/// Frames read ahead of the one being handled (blocks are up to 16 MiB).
const READ_AHEAD: usize = 4;

/// How receive talks to the person: in a terminal, or a test.
#[derive(Clone)]
pub struct Ui {
    /// Shows the receive code.
    pub code: Arc<dyn Fn(&ReceiveCode) + Send + Sync>,
    /// Says what's happening, a line at a time.
    pub say: Arc<dyn Fn(&str) + Send + Sync>,
    /// Asks the person whether the source's screen shows these digits.
    pub confirm: Arc<dyn Fn(Asked) -> Answer + Send + Sync>,
}

/// The person's answer, yes or no, when they give it.
pub type Answer = Pin<Box<dyn Future<Output = bool> + Send>>;

/// What the person is asked to confirm.
#[derive(Clone, Debug)]
pub struct Asked {
    pub digits: String,
    /// The source computer, and its platform ("macOS arm64").
    pub machine: String,
    pub platform: String,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// The person has seen the old computer say it moved: finish a move
    /// that's waiting for the source's commit without it.
    pub committed: bool,
    /// Only this computer's addresses, no relays (tests).
    pub local_only: bool,
}

fn receive_dir(home: &Home) -> PathBuf {
    dir(home).join("receive")
}

fn staging(home: &Home) -> PathBuf {
    dir(home).join("bundle")
}

/// Which source has the code, once both people confirmed the digits.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Claim {
    source: String,
    machine: String,
}

impl Claim {
    fn path(home: &Home) -> PathBuf {
        receive_dir(home).join("claim.json")
    }

    fn load(home: &Home) -> Option<Claim> {
        let text = std::fs::read_to_string(Claim::path(home)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn save(&self, home: &Home) -> Result<(), ImportError> {
        let text = serde_json::to_string(self).unwrap_or_default();
        crate::remote::write_private(&Claim::path(home), text.as_bytes()).map_err(|error| failed("Can't save who's moving here", error))
    }
}

/// The receive endpoint's key and the code's secret, made the first time.
fn key_and_secret(home: &Home) -> Result<(SecretKey, String), ImportError> {
    let dir = receive_dir(home);
    crate::remote::private_dir(&dir).map_err(|error| failed("Can't make the move's folder", error))?;
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok().and_then(|text| unhex(text.trim())).filter(|b| b.len() == 32);
    let key = match read("key") {
        Some(bytes) => SecretKey::from_bytes(&bytes.try_into().unwrap_or([0; 32])),
        None => {
            let key = SecretKey::generate();
            crate::remote::write_private(&dir.join("key"), format!("{}\n", hex(&key.to_bytes())).as_bytes())
                .map_err(|error| failed("Can't save the move's key", error))?;
            key
        }
    };
    let secret = match read("secret") {
        Some(bytes) => hex(&bytes),
        None => {
            let mut bytes = [0_u8; 32];
            getrandom::fill(&mut bytes).map_err(|error| failed("Can't make the receive code", error))?;
            crate::remote::write_private(&dir.join("secret"), format!("{}\n", hex(&bytes)).as_bytes())
                .map_err(|error| failed("Can't save the receive code", error))?;
            hex(&bytes)
        }
    };
    Ok((key, secret))
}

/// Starts receiving, or carries on: the checks, then this home's import
/// state (so its backend doesn't start meanwhile).
fn begin(home: &Home, addr: SocketAddr) -> Result<State, ImportError> {
    let state = match load_state(home)? {
        Some(state) if state.received => state,
        Some(state) => {
            return Err(ImportError::Refused(format!(
                "An import of {} into this Claktory hasn't finished. Finish it first (factoryd import with that bundle), or undo it with factoryd import --abandon.",
                state.bundle.display()
            )));
        }
        None => {
            if running(addr) && !super::moved::held(home) {
                return Err(running_here(addr));
            }
            fresh(home)?;
            local_disk(home.root())?;
            let state = State { received: true, bundle: staging(home), ..State::default() };
            save_state(home, &state)?;
            state
        }
    };
    not_running(home, addr)?;
    Ok(state)
}

/// Receives a Claktory into `home` until it's moved here (the text to show),
/// or the source cancels. Err when it can't.
pub async fn receive(home: &Home, addr: SocketAddr, options: &Options, ui: &Ui) -> Result<String, ImportError> {
    let _lock = lock(home)?;
    let result = receiving(home, addr, options, ui).await;
    if result.is_err() {
        forget_unclaimed(home);
    }
    result
}

/// After receive stops, by a failure or the person: when no source has
/// the code and nothing is switched, nothing is kept, and the backend can
/// start as before.
pub fn forget_unclaimed(home: &Home) {
    let Ok(Some(state)) = load_state(home) else { return };
    if state.received && state.switched.is_empty() && Claim::load(home).is_none() {
        let _ = std::fs::remove_dir_all(dir(home));
    }
}

async fn receiving(home: &Home, addr: SocketAddr, options: &Options, ui: &Ui) -> Result<String, ImportError> {
    let state = begin(home, addr)?;
    if options.committed {
        return commit(home, &state).map_err(ImportError::Refused);
    }
    let (key, secret) = key_and_secret(home)?;
    let endpoint = bind(key, options.local_only).await?;
    if !options.local_only {
        let _ = tokio::time::timeout(ONLINE_WAIT, endpoint.online()).await;
    }
    let code = ReceiveCode {
        endpoint: endpoint.id().to_string(),
        secret: secret.clone(),
        machine: crate::remote::machine_name().to_owned(),
        version: crate::update::VERSION.to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        relays: endpoint.addr().relay_urls().map(ToString::to_string).collect(),
        addresses: if options.local_only {
            endpoint.bound_sockets().into_iter().filter(SocketAddr::is_ipv4).collect()
        } else {
            endpoint.addr().ip_addrs().filter(|address| !address.ip().is_loopback()).copied().collect()
        },
    };
    (ui.code)(&code);
    let staged = {
        let home = home.clone();
        tokio::task::spawn_blocking(move || Staging::load(&home)).await.map_err(|error| failed("Can't read what's here", error))?
    };
    let claimed = Arc::new(std::sync::Mutex::new(Claim::load(home).map(|claim| claim.source)));
    let done = Arc::new(std::sync::Mutex::new(None));
    let mut ctx = Ctx {
        home: home.clone(),
        secret,
        target: code.endpoint.clone(),
        ui: ui.clone(),
        staging: staged,
        machine: String::new(),
        code,
        claimed: claimed.clone(),
        working: Arc::new(tokio::sync::Mutex::new(())),
        done: done.clone(),
    };
    // Connections are accepted apart from the session, so a slow handshake
    // never holds it up.
    let (arrived, mut connections) = mpsc::unbounded_channel::<Connection>();
    let accepting = {
        let endpoint = endpoint.clone();
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let arrived = arrived.clone();
                tokio::spawn(async move {
                    if let Ok(Ok(connection)) = tokio::time::timeout(HELLO_WITHIN, incoming).await {
                        let _ = arrived.send(connection);
                    }
                });
            }
        })
    };
    let mut next = None;
    let result = loop {
        let connection = match next.take() {
            Some(connection) => connection,
            None => match connections.recv().await {
                Some(connection) => connection,
                None => break Err(ImportError::Failed("The connection for the move closed.".to_owned())),
            },
        };
        let current = connection.clone();
        let outcome = {
            let session = ctx.session(connection);
            tokio::pin!(session);
            loop {
                tokio::select! {
                    outcome = &mut session => break Some(outcome),
                    Some(other) = connections.recv() => {
                        // The source that has the code, back (it restarted,
                        // or its connection broke without a word): the same
                        // move carrying on, in place of the session before.
                        let ours = locked(&claimed).as_deref() == Some(other.remote_id().to_string().as_str());
                        if ours {
                            next = Some(other);
                            break None;
                        }
                        tokio::spawn(turn_away(other));
                    }
                }
            }
        };
        if outcome.is_none() {
            current.close(0u32.into(), b"replaced");
        }
        match outcome {
            Some(Outcome::Over(result)) => break result,
            Some(Outcome::Done(text)) => break Ok(text),
            Some(Outcome::Again) | None => {}
        }
        if next.is_none()
            && let Some(text) = locked(&done).clone()
        {
            break Ok(text);
        }
    };
    accepting.abort();
    endpoint.close().await;
    result
}

async fn bind(key: SecretKey, local_only: bool) -> Result<Endpoint, ImportError> {
    let cant = |error: &dyn std::fmt::Display| ImportError::Failed(format!("Can't start the connection for the move: {error}"));
    let builder = if local_only {
        Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::Disabled)
            .clear_ip_transports()
            .bind_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .map_err(|error| cant(&error))?
    } else {
        // n0's address lookup and n0's relays: Claktory's forwards only
        // to or from a signed-in Claktory's tunnel key (#450), and neither
        // end of a move is one.
        Endpoint::builder(presets::N0).relay_mode(RelayMode::Custom(crate::remote::public_relay_map()))
    };
    builder.secret_key(key).alpns(vec![move_wire::ALPN.to_vec()]).bind().await.map_err(|error| cant(&error))
}

/// A second source while one is moving here: told so, and let go.
async fn turn_away(connection: Connection) {
    let Ok((mut send, _receive)) = connection.accept_bi().await else { return };
    let why = "Another computer is moving its Claktory here right now.".to_owned();
    let _ = move_wire::write(&mut send, &Frame::Failed { why }, &[]).await;
    let _ = send.finish();
    let _ = tokio::time::timeout(LINGER, connection.closed()).await;
}

/// The move as it was finished, the source having committed (or the person
/// saying so with `--committed`): the import completes, and the text for
/// the person.
fn commit(home: &Home, state: &State) -> Result<String, String> {
    if !state.received || state.step != Step::Commit {
        return Err("Nothing here is waiting for the old computer to hand over.".to_owned());
    }
    let manifest = read_manifest(&staging(home)).map_err(|error| error.to_string())?;
    let imported = report(home, &manifest, &PathMap::new(state.map.clone()), state);
    finish(home, &manifest, state).map_err(|error| error.to_string())?;
    Ok(imported.text)
}

/// How a session with a source ended.
enum Outcome {
    /// Wait for it, or another, to connect again.
    Again,
    /// Receive is over.
    Over(Result<String, ImportError>),
    /// Committed: over, once the source has heard `done`.
    Done(String),
}

type Out = mpsc::UnboundedSender<(Frame, Vec<u8>)>;
type In = mpsc::Receiver<Result<(Frame, Vec<u8>), move_wire::WireError>>;

struct Ctx {
    home: Home,
    secret: String,
    /// This endpoint's id.
    target: String,
    ui: Ui,
    staging: Staging,
    /// The source's name, once it said hello.
    machine: String,
    /// The code shown.
    code: ReceiveCode,
    /// The source key that has the code, once both people confirmed.
    claimed: Arc<std::sync::Mutex<Option<String>>>,
    /// Held while the import or the commit runs, which go on even when the
    /// session that started them is replaced: the next one waits for them.
    working: Arc<tokio::sync::Mutex<()>>,
    /// The text for the person once the source committed and this side
    /// finished: the move is done, whatever happens to the session.
    done: Arc<std::sync::Mutex<Option<String>>>,
}

/// Writes what's queued on `out`, in order, while the session reads.
fn writer(mut send: SendStream) -> (Out, tokio::task::JoinHandle<()>) {
    let (out, mut queued) = mpsc::unbounded_channel::<(Frame, Vec<u8>)>();
    let task = tokio::spawn(async move {
        while let Some((frame, bytes)) = queued.recv().await {
            if move_wire::write(&mut send, &frame, &bytes).await.is_err() {
                break;
            }
        }
        let _ = send.finish();
    });
    (out, task)
}

/// Reads frames ahead into a channel, so waiting for one never loses half
/// of another.
fn reader(mut receive: RecvStream) -> In {
    let (frames, read) = mpsc::channel(READ_AHEAD);
    tokio::spawn(async move {
        loop {
            let frame = move_wire::read(&mut receive).await;
            let end = frame.is_err();
            if frames.send(frame).await.is_err() || end {
                break;
            }
        }
    });
    read
}

impl Ctx {
    fn say(&self, text: &str) {
        (self.ui.say)(text);
    }

    /// After pairing failed: a new secret, so whoever saw the old code
    /// can't ask again, and the new code shown.
    fn new_code(&mut self) {
        let mut bytes = [0_u8; 32];
        if getrandom::fill(&mut bytes).is_err() {
            return;
        }
        let secret = hex(&bytes);
        if crate::remote::write_private(&receive_dir(&self.home).join("secret"), format!("{secret}\n").as_bytes()).is_err() {
            return;
        }
        self.secret.clone_from(&secret);
        self.code.secret = secret;
        self.say("The code before no longer works. To try again, use this new one:");
        (self.ui.code)(&self.code);
    }

    async fn session(&mut self, connection: Connection) -> Outcome {
        let Ok((send, receive)) = connection.accept_bi().await else { return Outcome::Again };
        let (out, written) = writer(send);
        let mut frames = reader(receive);
        let source = connection.remote_id().to_string();
        let outcome = self.talk(&source, &out, &mut frames).await;
        drop(out);
        let _ = written.await;
        if matches!(outcome, Outcome::Done(_)) {
            let _ = tokio::time::timeout(DONE_LINGER, connection.closed()).await;
        } else {
            // Its last frame has the time to be read, without holding up the
            // next session: a source starting over at once (a move called
            // off at the digits, then started again) is that session, not
            // turned away as another computer.
            tokio::spawn(async move {
                let _ = tokio::time::timeout(LINGER, connection.closed()).await;
            });
        }
        outcome
    }

    /// Says why to the source, and to the person; the session ends.
    fn fail(&self, out: &Out, why: String) -> Outcome {
        self.say(&why);
        let _ = out.send((Frame::Failed { why }, Vec::new()));
        Outcome::Again
    }

    async fn talk(&mut self, source: &str, out: &Out, frames: &mut In) -> Outcome {
        let hello = match tokio::time::timeout(HELLO_WITHIN, frames.recv()).await {
            Ok(Some(Ok((hello, _)))) => hello,
            _ => return Outcome::Again,
        };
        let Frame::Hello { secret, version, machine, os, arch } = hello else {
            return Outcome::Again;
        };
        if !move_wire::same(&secret, &self.secret) {
            tokio::time::sleep(WRONG_PAUSE).await;
            let why = "That isn't the code this computer shows now: copy it again from where factoryd receive runs.".to_owned();
            let _ = out.send((Frame::Failed { why }, Vec::new()));
            return Outcome::Again;
        }
        let claim = locked(&self.claimed).clone();
        let resumed = claim.as_deref() == Some(source);
        // Another key while the one that has the code isn't connected: the
        // same computer starting its move again (a source starts each try
        // with a new key, #23), let in only by the digits once more.
        let again = claim.is_some() && !resumed;
        if again && let Err(why) = self.may_start_again(&machine) {
            return self.fail(out, why);
        }
        let here = crate::update::VERSION;
        if too_new(&version, here) {
            return self.fail(
                out,
                format!("This computer has Claktory {here}, older than {machine}'s ({version}). Install the latest Claktory here, then run factoryd receive again."),
            );
        }
        self.machine.clone_from(&machine);
        let welcome = Frame::Welcome {
            version: here.to_owned(),
            machine: crate::remote::machine_name().to_owned(),
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            home: self.home.root().display().to_string(),
            user_home: std::env::home_dir().map(|path| path.display().to_string()).unwrap_or_default(),
            free_bytes: super::free_bytes(self.home.root()).unwrap_or_default(),
            local_disk: true,
            fresh: true,
            resumed,
        };
        let _ = out.send((welcome, Vec::new()));
        if resumed {
            self.say(&format!("{machine} is back: carrying on."));
        } else {
            if again {
                self.say(&format!(
                    "{machine} is starting its move again. Confirm only if {machine}'s own screen shows the same digits; it then carries on with what's here."
                ));
            }
            let asked = Asked {
                digits: move_wire::digits(&self.target, source, &self.secret),
                machine: machine.clone(),
                platform: platform(&os, &arch),
            };
            if let Err(outcome) = self.pair(asked, out, frames).await {
                self.new_code();
                return outcome;
            }
            // Started again: it sends its plan and rounds afresh, onto
            // what's staged.
            if again && let Err(why) = self.start_again() {
                return self.fail(out, why);
            }
            if let Err(error) = (Claim { source: source.to_owned(), machine: machine.clone() }).save(&self.home) {
                return self.fail(out, error.to_string());
            }
            *locked(&self.claimed) = Some(source.to_owned());
            self.say(&format!("Confirmed: {machine} is moving its Claktory here."));
        }
        // A move that got further before: the import carries on, or the
        // source is told again that it's ready.
        let step = load_state(&self.home).ok().flatten().map(|state| state.step).unwrap_or_default();
        if step > Step::Unpack
            && let Some(outcome) = self.import(out).await
        {
            return outcome;
        }
        while let Some(frame) = frames.recv().await {
            let Ok((frame, bytes)) = frame else { break };
            if let Some(outcome) = self.handle(frame, bytes, out).await {
                return outcome;
            }
        }
        self.staging.save();
        self.say(&format!("The connection with {machine} dropped. Waiting for it to come back…"));
        Outcome::Again
    }

    /// Whether `machine`, saying hello with another key than the one that
    /// has the code, may start the move again (#23): only the computer that
    /// had it, by its name, and only while nothing here is switched over (a
    /// source that got that far commits with the key it had). Its digits are
    /// confirmed again either way, as for a first try.
    fn may_start_again(&self, machine: &str) -> Result<(), String> {
        let refused = "Another computer's Claktory is already moving here, so this one can't. To start over here, stop factoryd receive, run factoryd import --abandon, then factoryd receive again.";
        let claimed = Claim::load(&self.home).map(|claim| claim.machine).unwrap_or_default();
        if claimed.is_empty() || claimed != machine {
            return Err(refused.to_owned());
        }
        let state = load_state(&self.home).map_err(|error| error.to_string())?.unwrap_or_default();
        if !state.switched.is_empty() || state.step > Step::Prepare {
            return Err(format!(
                "{machine}'s move here got as far as putting its Claktory in place, so it can't start again. To start over here, stop factoryd receive, run factoryd import --abandon, then factoryd receive again."
            ));
        }
        Ok(())
    }

    /// The move starts again from its plan: what's staged stays, for the
    /// rounds to come to compare with.
    fn start_again(&self) -> Result<(), String> {
        let mut state = load_state(&self.home).map_err(|error| error.to_string())?.unwrap_or_default();
        if !state.switched.is_empty() {
            return Err("This computer is already putting the Claktory in place, so the move can't start again.".to_owned());
        }
        state.step = Step::Unpack;
        save_state(&self.home, &state).map_err(|error| error.to_string())
    }

    /// Both people confirm the digits; Err when either doesn't.
    async fn pair(&self, asked: Asked, out: &Out, frames: &mut In) -> Result<(), Outcome> {
        let machine = asked.machine.clone();
        let confirmed = (self.ui.confirm)(asked);
        tokio::pin!(confirmed);
        let (mut ours, mut theirs) = (false, false);
        while !(ours && theirs) {
            tokio::select! {
                yes = &mut confirmed, if !ours => {
                    if !yes {
                        return Err(self.fail(out, format!("The digits weren't confirmed on {}.", crate::remote::machine_name())));
                    }
                    ours = true;
                    let _ = out.send((Frame::Confirmed {}, Vec::new()));
                    if !theirs {
                        self.say(&format!("Confirmed here. Waiting for {machine} to confirm too…"));
                    }
                }
                frame = frames.recv() => match frame {
                    Some(Ok((Frame::Confirmed {}, _))) => theirs = true,
                    Some(Ok((Frame::Failed { why }, _))) => {
                        self.say(&format!("{machine}: {why}"));
                        return Err(Outcome::Again);
                    }
                    Some(Ok((Frame::Unknown, _))) => {}
                    Some(Ok(_)) => return Err(self.fail(out, "The digits have to be confirmed first.".to_owned())),
                    _ => {
                        self.say(&format!("{machine} went away before the digits were confirmed."));
                        return Err(Outcome::Again);
                    }
                }
            }
        }
        Ok(())
    }

    /// One frame after pairing; Some when the session ends.
    async fn handle(&mut self, frame: Frame, bytes: Vec<u8>, out: &Out) -> Option<Outcome> {
        match frame {
            Frame::Plan { manifest, map, tools } => {
                let checked = self.check(manifest, &map, &tools);
                match checked {
                    Ok(checked) => {
                        let _ = out.send((checked, Vec::new()));
                    }
                    Err(why) => return Some(self.fail(out, why)),
                }
            }
            Frame::Round { last, .. } => {
                if let Err(why) = self.round_starts() {
                    return Some(self.fail(out, why));
                }
                self.staging.start(last);
            }
            Frame::Entry(entry) => {
                let path = entry.path.clone();
                match self.staging.entry(entry) {
                    Ok(blocks) if blocks.is_empty() => {}
                    Ok(blocks) => {
                        let _ = out.send((Frame::Need { path, blocks }, Vec::new()));
                    }
                    Err(why) => return Some(self.fail(out, why)),
                }
            }
            Frame::RoundEnd { n } => {
                let _ = out.send((Frame::NeedEnd { n }, Vec::new()));
            }
            Frame::Block { path, index, .. } => {
                if let Err(why) = self.staging.block(&path, index, &bytes) {
                    return Some(self.fail(out, why));
                }
            }
            Frame::BlocksEnd { n } => {
                let done = match self.staging.finish_round() {
                    Ok(done) => done,
                    Err(why) => return Some(self.fail(out, why)),
                };
                let _ = out.send((Frame::RoundDone { n, bytes_have: done.have, bytes_total: done.total, stale: done.stale }, Vec::new()));
                self.say(&format!("{} MB of {} MB here.", done.have >> 20, done.total >> 20));
                if done.last {
                    if !done.whole {
                        return Some(self.fail(out, "The last copy didn't arrive whole: some files are missing parts.".to_owned()));
                    }
                    return self.import(out).await;
                }
            }
            Frame::Commit { export_id, .. } => {
                let working = self.working.clone().lock_owned().await;
                if let Some(text) = locked(&self.done).clone() {
                    // Committed before; the source didn't hear it.
                    let _ = out.send((Frame::Done {}, Vec::new()));
                    return Some(Outcome::Done(text));
                }
                let state = load_state(&self.home).ok().flatten().unwrap_or_default();
                if !export_id.is_empty() && export_id != state.export_id {
                    return Some(self.fail(out, "That isn't the move this computer is ready for.".to_owned()));
                }
                let (home, done) = (self.home.clone(), self.done.clone());
                let committed = tokio::task::spawn_blocking(move || {
                    let _working = working;
                    let committed = commit(&home, &state);
                    if let Ok(text) = &committed {
                        *locked(&done) = Some(text.clone());
                    }
                    committed
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
                return Some(match committed {
                    Ok(text) => {
                        let _ = out.send((Frame::Done {}, Vec::new()));
                        Outcome::Done(text)
                    }
                    Err(why) => self.fail(out, why),
                });
            }
            Frame::Cancel { why } => {
                let home = self.home.clone();
                let undone = tokio::task::spawn_blocking(move || undo(&home)).await;
                let text = format!(
                    "{} called the move off.{} Nothing changed here.",
                    self.machine,
                    reason(&why, &[&self.code.machine, &self.machine])
                );
                // Not moved: an end of its own, never a success.
                return Some(Outcome::Over(match undone {
                    Ok(Ok(_)) => Err(ImportError::CalledOff(text)),
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(failed("Can't undo the move", error)),
                }));
            }
            Frame::Failed { why } => {
                self.say(&format!("{}: {why}", self.machine));
                return Some(Outcome::Again);
            }
            _ => {}
        }
        None
    }

    /// The target's checks of the source's plan, and the paths it settles.
    fn check(&mut self, manifest: serde_json::Value, map: &[(String, String)], tools: &[String]) -> Result<Frame, String> {
        let manifest: Manifest = serde_json::from_value(manifest).map_err(|error| format!("The move's plan can't be read: {error}"))?;
        let mut found = Vec::new();
        if manifest.format != FORMAT {
            found.push(format!(
                "{} moves in format {}, which this version of Claktory ({}) can't read. Install the latest Claktory here.",
                manifest.source.machine,
                manifest.format,
                crate::update::VERSION
            ));
        }
        let mut given = Vec::new();
        for (from, to) in map {
            match super::parse_pair(&format!("{from}={to}")) {
                Ok(pair) => given.push(pair),
                Err(why) => found.push(why),
            }
        }
        let map = path_map(&self.home, &manifest, &given);
        found.extend(problems(&self.home, &manifest, &map, None, self.staging.bytes(), true).map_err(|error| error.to_string())?);
        let missing: Vec<String> = tools.iter().filter(|tool| !on_path(tool)).cloned().collect();
        let mut state = load_state(&self.home).map_err(|error| error.to_string())?.unwrap_or_default();
        if state.switched.is_empty() {
            state.map = map.pairs().to_vec();
            state.export_id.clone_from(&manifest.export_id);
            save_state(&self.home, &state).map_err(|error| error.to_string())?;
        }
        let pairs = map.pairs().iter().map(|(from, to)| (from.display().to_string(), to.display().to_string())).collect();
        Ok(Frame::Checked { problems: found, map: pairs, missing, free_bytes: super::free_bytes(self.home.root()).unwrap_or_default() })
    }

    /// A round can start while nothing is switched over: an import that
    /// stopped before then starts again from what's staged.
    fn round_starts(&self) -> Result<(), String> {
        let mut state = load_state(&self.home).map_err(|error| error.to_string())?.unwrap_or_default();
        if state.step == Step::Unpack {
            return Ok(());
        }
        if !state.switched.is_empty() {
            return Err("This computer is already putting the Claktory in place, so it takes no more copies.".to_owned());
        }
        state.step = Step::Unpack;
        save_state(&self.home, &state).map_err(|error| error.to_string())
    }

    /// Imports what's staged, up to Verify; Some when the session ends.
    async fn import(&mut self, out: &Out) -> Option<Outcome> {
        // An import a session before started (one since replaced) finishes
        // first; this one carries on from where it got.
        let working = self.working.clone().lock_owned().await;
        let mut state = load_state(&self.home).ok().flatten()?;
        if state.step == Step::Unpack {
            state.step = Step::Prepare;
            if let Err(error) = save_state(&self.home, &state) {
                return Some(self.fail(out, error.to_string()));
            }
        }
        if state.step == Step::Commit {
            let _ = out.send((Frame::Ready { old_app_code: state.old_app_code.clone() }, Vec::new()));
            return None;
        }
        let (home, say, progress) = (self.home.clone(), self.ui.say.clone(), out.clone());
        let imported = tokio::task::spawn_blocking(move || {
            let _working = working;
            steps(&home, state, &[], true, &mut |step, text| {
                say(text);
                let _ = progress.send((Frame::Progress { step: step.to_owned(), text: text.to_owned() }, Vec::new()));
            })
        })
        .await
        .unwrap_or_else(|error| Err(failed("The import stopped", error)));
        match imported {
            Ok(imported) => {
                let _ = out.send((Frame::Ready { old_app_code: imported.old_app_code }, Vec::new()));
                self.say(&format!("Verified. Waiting for {} to hand over…", self.machine));
                None
            }
            Err(error) => {
                let mut state = load_state(&self.home).ok().flatten().unwrap_or_default();
                if state.switched.is_empty() {
                    // Nothing here changed: what's staged waits for another try.
                    state.step = Step::Unpack;
                    let _ = save_state(&self.home, &state);
                    return Some(self.fail(out, error.to_string()));
                }
                let home = self.home.clone();
                let undone = tokio::task::spawn_blocking(move || undo(&home)).await;
                let why = match undone {
                    Ok(Ok(_)) => format!("{error} This computer is as it was before: run factoryd receive again to try once more."),
                    _ => format!("{error} Undo it with factoryd import --abandon, then run factoryd receive again."),
                };
                self.fail(out, why.clone());
                Some(Outcome::Over(Err(ImportError::Failed(why))))
            }
        }
    }
}

/// The source's reason, as a sentence after "… called the move off.", or
/// nothing when it only says that again.
fn reason(why: &str, names: &[&str]) -> String {
    let why = why.trim();
    if why.is_empty() || why.to_lowercase().contains("called off") {
        return String::new();
    }
    let end = if why.ends_with(['.', '!', '?']) { "" } else { "." };
    // A computer's name keeps its own case ("matts-mac can't take this
    // Claktory"); anything else starts as a sentence does.
    if names.iter().any(|name| !name.is_empty() && why.starts_with(name)) {
        return format!(" {why}{end}");
    }
    let mut chars = why.chars();
    let first = chars.next().map(|first| first.to_uppercase().collect::<String>()).unwrap_or_default();
    format!(" {first}{}{end}", chars.as_str())
}

// ---- what's staged ----

/// A staged file: what its blocks hold now, as of its size and time.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Staged {
    size: u64,
    modified_ns: u64,
    block: u64,
    /// Each block's SHA-256, "" when it isn't known to be right.
    hashes: Vec<String>,
}

/// The listing of the round under way.
#[derive(Default)]
struct Round {
    last: bool,
    listed: BTreeMap<String, Entry>,
    needed: BTreeMap<String, BTreeSet<u32>>,
    stale: u32,
}

/// What a round came to.
struct RoundDone {
    have: u64,
    total: u64,
    stale: u32,
    last: bool,
    /// Every listed file is whole.
    whole: bool,
}

/// The tree being received, in `<home>/import/bundle`, and its index.
struct Staging {
    root: PathBuf,
    index_path: PathBuf,
    index: BTreeMap<String, Staged>,
    round: Option<Round>,
    /// Where a block of each hash is, at the round's start: (path, index).
    by_hash: HashMap<String, (String, u64)>,
}

/// Opens the staged file `path` in `root` to write, never through a link:
/// none of the folders above it, nor it, may be one (a later entry could
/// have made one since it was listed).
fn open_in(root: &Path, path: &str) -> Result<std::fs::File, String> {
    let mut at = root.to_path_buf();
    let parts: Vec<&str> = path.split('/').collect();
    for part in &parts[..parts.len() - 1] {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.is_dir() => {}
            _ => return Err(format!("{path} would be written through a link, which isn't allowed.")),
        }
    }
    let file = root.join(path);
    writable(&file).map_err(|error| format!("Can't write {path} on this computer: {error}"))?;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(file)
        .map_err(|error| format!("Can't write {path} on this computer: {error}"))
}

/// A staged file given the source's mode when a round finished, read-only
/// say, made the owner's to read and write again, for the next round to
/// write (#21); a round's end gives it the source's mode again. Never
/// through a link: only a plain file is changed.
fn writable(file: &Path) -> std::io::Result<()> {
    let meta = match std::fs::symlink_metadata(file) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mode = meta.permissions().mode() & 0o777;
    if meta.is_file() && mode & 0o600 != 0o600 {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(mode | 0o600))?;
    }
    Ok(())
}

/// Whether the link at `path` in `root`, to `target`, resolves inside
/// `root` as the tree is now: no folder above it is a link, and its target
/// passes through no other link on the way (a chain such as `b -> .`,
/// `a -> b/../..` climbs out though each link alone looks inside).
fn link_stays_inside(root: &Path, path: &str, target: &str) -> bool {
    if !move_wire::safe_link(path, target) {
        return false;
    }
    let mut at: Vec<&str> = path.split('/').collect();
    at.pop();
    for depth in 1..=at.len() {
        if std::fs::symlink_metadata(root.join(at[..depth].join("/"))).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return false;
        }
    }
    let parts: Vec<&str> = target.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    for (index, part) in parts.iter().enumerate() {
        if *part == ".." {
            at.pop();
            continue;
        }
        at.push(part);
        let last = index == parts.len() - 1;
        if !last && std::fs::symlink_metadata(root.join(at.join("/"))).is_ok_and(|meta| meta.file_type().is_symlink()) {
            return false;
        }
    }
    true
}

fn modified_ns(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|at| at.duration_since(UNIX_EPOCH).ok()).map_or(0, |at| at.as_nanos() as u64)
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

impl Staging {
    /// What's staged in `home`, checked against the files as they are.
    fn load(home: &Home) -> Staging {
        let root = staging(home);
        let index_path = receive_dir(home).join("index.json");
        let saved: BTreeMap<String, Staged> =
            std::fs::read_to_string(&index_path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        let mut index = BTreeMap::new();
        for (path, mut staged) in saved {
            let file = root.join(&path);
            let Ok(meta) = std::fs::symlink_metadata(&file) else { continue };
            if !meta.is_file() {
                continue;
            }
            if meta.len() != staged.size || modified_ns(&meta) != staged.modified_ns {
                let Ok(hashes) = move_wire::hash_file(&file, staged.block) else { continue };
                staged = Staged { size: meta.len(), modified_ns: modified_ns(&meta), block: staged.block, hashes };
            }
            index.insert(path, staged);
        }
        Staging { root, index_path, index, round: None, by_hash: HashMap::new() }
    }

    fn save(&self) {
        if let Ok(text) = serde_json::to_string(&self.index) {
            let _ = crate::remote::write_private(&self.index_path, text.as_bytes());
        }
    }

    /// Bytes staged.
    fn bytes(&self) -> u64 {
        self.index.values().map(|staged| staged.size).sum()
    }

    fn start(&mut self, last: bool) {
        self.round = Some(Round { last, ..Round::default() });
        self.by_hash.clear();
        for (path, staged) in &self.index {
            for (at, hash) in staged.hashes.iter().enumerate() {
                if !hash.is_empty() {
                    self.by_hash.entry(hash.clone()).or_insert_with(|| (path.clone(), at as u64 * staged.block));
                }
            }
        }
    }

    /// Where `path` goes, with the folders above it made: never through a
    /// link, which could lead out of the tree.
    fn place(&self, path: &str) -> Result<PathBuf, String> {
        let mut at = self.root.clone();
        let parts: Vec<&str> = path.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            at.push(part);
            match std::fs::symlink_metadata(&at) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(format!("{path} would be written through a link, which isn't allowed."));
                }
                Ok(meta) if meta.is_dir() => continue,
                Ok(_) => std::fs::remove_file(&at).map_err(|error| self.cant(path, &error))?,
                Err(_) => {}
            }
            std::fs::create_dir(&at).map_err(|error| self.cant(path, &error))?;
            let _ = std::fs::set_permissions(&at, std::fs::Permissions::from_mode(0o700));
        }
        Ok(self.root.join(path))
    }

    fn open(&self, path: &str) -> Result<std::fs::File, String> {
        open_in(&self.root, path)
    }

    fn cant(&self, path: &str, error: &std::io::Error) -> String {
        format!("Can't write {path} on this computer: {error}")
    }

    /// One path of the round's listing: made, or compared with what's
    /// staged; the blocks of it that are needed.
    fn entry(&mut self, entry: Entry) -> Result<Vec<u32>, String> {
        if self.round.is_none() {
            return Err("A listing came outside a round.".to_owned());
        }
        if !move_wire::safe_path(&entry.path) {
            return Err(format!("The move holds a path that isn't allowed ({}).", entry.path));
        }
        std::fs::create_dir_all(&self.root).map_err(|error| self.cant("the move's folder", &error))?;
        let target = self.place(&entry.path)?;
        let existing = std::fs::symlink_metadata(&target).ok();
        let mut needed = Vec::new();
        match entry.kind {
            Kind::Dir => {
                if existing.as_ref().is_some_and(|meta| !meta.is_dir()) {
                    remove_any(&target).map_err(|error| self.cant(&entry.path, &error))?;
                    self.index.remove(&entry.path);
                }
                std::fs::create_dir_all(&target).map_err(|error| self.cant(&entry.path, &error))?;
            }
            Kind::Symlink => {
                if !move_wire::safe_link(&entry.path, &entry.target) {
                    return Err(format!("The move holds a link that leads out of it ({} → {}).", entry.path, entry.target));
                }
                let same = std::fs::read_link(&target).ok().is_some_and(|now| now == Path::new(&entry.target));
                if !same {
                    remove_any(&target).map_err(|error| self.cant(&entry.path, &error))?;
                    self.index.remove(&entry.path);
                    std::os::unix::fs::symlink(&entry.target, &target).map_err(|error| self.cant(&entry.path, &error))?;
                }
            }
            Kind::File => {
                let block = entry.block;
                let fits = block.is_power_of_two()
                    && (move_wire::MIN_BLOCK..=move_wire::MAX_BLOCK).contains(&block)
                    && entry.hashes.len() as u64 == move_wire::block_count(entry.size, block);
                if !fits {
                    return Err(format!("The move listed {} with blocks that don't add up.", entry.path));
                }
                if existing.as_ref().is_some_and(|meta| !meta.is_file()) {
                    remove_any(&target).map_err(|error| self.cant(&entry.path, &error))?;
                    self.index.remove(&entry.path);
                }
                writable(&target).map_err(|error| self.cant(&entry.path, &error))?;
                let file = std::fs::OpenOptions::new()
                    .write(true)
                    .read(true)
                    .create(true)
                    .truncate(false)
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(&target)
                    .map_err(|error| self.cant(&entry.path, &error))?;
                let mut staged = self.index.remove(&entry.path).filter(|staged| staged.block == block).unwrap_or_default();
                staged.block = block;
                staged.hashes.resize(entry.hashes.len(), String::new());
                for (at, hash) in entry.hashes.iter().enumerate() {
                    if staged.hashes[at] == *hash {
                        continue;
                    }
                    let length = block.min(entry.size - at as u64 * block);
                    if self.copy_block(&entry.path, hash, length, &file, at as u64 * block) {
                        staged.hashes[at].clone_from(hash);
                        continue;
                    }
                    staged.hashes[at].clear();
                    needed.push(at as u32);
                }
                self.index.insert(entry.path.clone(), staged);
            }
        }
        let round = self.round.as_mut().unwrap_or_else(|| unreachable!());
        round.needed.insert(entry.path.clone(), needed.iter().copied().collect());
        round.listed.insert(entry.path.clone(), entry);
        Ok(needed)
    }

    /// Copies a block with this hash from another staged file, when one has
    /// it (a file renamed, say).
    fn copy_block(&self, path: &str, hash: &str, length: u64, into: &std::fs::File, offset: u64) -> bool {
        let Some((from, from_offset)) = self.by_hash.get(hash).filter(|(from, _)| from != path) else { return false };
        let Ok(source) = self.open(from) else { return false };
        let mut bytes = vec![0; length as usize];
        source.read_exact_at(&mut bytes, *from_offset).is_ok()
            && move_wire::block_hash(&bytes) == hash
            && into.write_all_at(&bytes, offset).is_ok()
    }

    /// A block the source sent, checked against the round's listing.
    fn block(&mut self, path: &str, index: u32, bytes: &[u8]) -> Result<(), String> {
        let Some(round) = self.round.as_mut() else { return Err("A block came outside a round.".to_owned()) };
        let Some(entry) = round.listed.get(path) else { return Err(format!("A block came for {path}, which isn't listed.")) };
        if !round.needed.get(path).is_some_and(|needed| needed.contains(&index)) {
            return Ok(());
        }
        let expected = &entry.hashes[index as usize];
        if bytes.len() as u64 > entry.block || move_wire::block_hash(bytes) != *expected {
            if round.last {
                return Err(format!("{path} arrived damaged."));
            }
            // It changed since it was listed: the next round has it.
            round.stale += 1;
            return Ok(());
        }
        let length = entry.block.min(entry.size - u64::from(index) * entry.block);
        if bytes.len() as u64 != length {
            return Err(format!("A block of {path} came the wrong size."));
        }
        let file = open_in(&self.root, path)?;
        file.write_all_at(bytes, u64::from(index) * entry.block)
            .map_err(|error| format!("Can't write {path} on this computer: {error}"))?;
        if let Some(staged) = self.index.get_mut(path) {
            staged.hashes[index as usize].clone_from(expected);
        }
        if let Some(needed) = round.needed.get_mut(path) {
            needed.remove(&index);
        }
        Ok(())
    }

    /// Ends the round: files cut to size, whole ones given their mode and
    /// time; after the final round, what isn't listed goes.
    fn finish_round(&mut self) -> Result<RoundDone, String> {
        let Some(round) = self.round.take() else { return Err("A round ended that didn't start.".to_owned()) };
        let (mut have, mut total, mut whole) = (0, 0, true);
        for (path, entry) in &round.listed {
            let target = self.root.join(path);
            match entry.kind {
                Kind::File => {
                    total += entry.size;
                    let file = self.open(path)?;
                    file.set_len(entry.size).map_err(|error| self.cant(path, &error))?;
                    let staged = self.index.entry(path.clone()).or_default();
                    let complete = staged.hashes == entry.hashes;
                    if complete {
                        have += entry.size;
                        // Never setuid, setgid or sticky: a Claktory moves no such file.
                        let mode = if entry.mode == 0 { 0o600 } else { entry.mode & 0o777 };
                        let _ = file.set_permissions(std::fs::Permissions::from_mode(mode));
                        let at = UNIX_EPOCH + Duration::from_millis(u64::try_from(entry.mtime_ms).unwrap_or(0));
                        let _ = file.set_modified(at.max(UNIX_EPOCH));
                    } else {
                        whole = false;
                        have += staged
                            .hashes
                            .iter()
                            .enumerate()
                            .filter(|(at, hash)| **hash == entry.hashes[*at])
                            .map(|(at, _)| entry.block.min(entry.size - at as u64 * entry.block))
                            .sum::<u64>();
                    }
                    drop(file);
                    if let Ok(meta) = std::fs::symlink_metadata(&target) {
                        staged.size = meta.len();
                        staged.modified_ns = modified_ns(&meta);
                    }
                }
                Kind::Dir => {
                    let mode = if entry.mode == 0 { 0o700 } else { (entry.mode & 0o777) | 0o700 };
                    if std::fs::symlink_metadata(&target).is_ok_and(|meta| meta.is_dir()) {
                        let _ = std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode));
                    }
                }
                Kind::Symlink => {}
            }
        }
        if round.last {
            for (path, entry) in &round.listed {
                if entry.kind == Kind::Symlink && !link_stays_inside(&self.root, path, &entry.target) {
                    return Err(format!("The move holds a link that leads out of it through another link ({path} → {}).", entry.target));
                }
            }
            self.prune(&round.listed).map_err(|error| format!("Can't clear what the move no longer has: {error}"))?;
        }
        self.save();
        Ok(RoundDone { have, total, stale: round.stale, last: round.last, whole })
    }

    /// Removes what's staged but not in `listed` (nor above what is).
    fn prune(&mut self, listed: &BTreeMap<String, Entry>) -> std::io::Result<()> {
        let mut keep: BTreeSet<String> = BTreeSet::new();
        for path in listed.keys() {
            let mut at = String::new();
            for part in path.split('/') {
                if !at.is_empty() {
                    at.push('/');
                }
                at.push_str(part);
                keep.insert(at.clone());
            }
        }
        fn walk(dir: &Path, prefix: &str, keep: &BTreeSet<String>, listed: &BTreeMap<String, Entry>) -> std::io::Result<()> {
            for item in std::fs::read_dir(dir)? {
                let item = item?;
                let name = item.file_name().to_string_lossy().into_owned();
                let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
                if !keep.contains(&path) {
                    remove_any(&item.path())?;
                } else if item.file_type()?.is_dir() && listed.get(&path).is_none_or(|entry| entry.kind == Kind::Dir) {
                    walk(&item.path(), &path, keep, listed)?;
                }
            }
            Ok(())
        }
        if self.root.is_dir() {
            walk(&self.root, "", &keep, listed)?;
        }
        self.index.retain(|path, _| listed.get(path).is_some_and(|entry| entry.kind == Kind::File));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::reason;

    #[test]
    fn a_reason_reads_as_one_sentence_and_isnt_said_twice() {
        assert_eq!(reason("The move was called off.", &[]), "");
        assert_eq!(reason("", &[]), "");
        assert_eq!(reason("the person changed their mind", &[]), " The person changed their mind.");
        assert_eq!(reason("Not enough room!", &[]), " Not enough room!");
        assert_eq!(
            reason("matts-mac can't take this Claktory: it's full", &["matts-mac"]),
            " matts-mac can't take this Claktory: it's full.",
            "a computer's name keeps its case"
        );
    }
}
