//! The runner on Linux (#435 P4): `factoryd browser-runner`, copied to
//! /opt/claktory-browser/runner when it differs, then run as the browser's
//! own user by the browser's gate, through the one sudo line setup
//! installed (C4). The gate runs it only once factoryd has signed its
//! challenge with the hash of this factoryd (`browser_runner::gate`). Its
//! stdin and stdout carry C1; nothing else is shared: sudo closes every
//! other file descriptor, and the gate gives it no environment but
//! SUDO_UID.
//!
//! What it says is a hostile page's after a Chrome escape: each frame's
//! length is checked against its channel's cap before anything is read
//! (`wire::parse_header`), and its stderr is read capped, cleaned and
//! only summarised in the log, never as it is (Fugo 36598).

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::Io;
use super::setup::{GATE, RUNNER, USER};
use crate::browser_runner::gate;
use crate::browser_runner::wire::{self, Channel, Way};
use crate::factory::Factory;

/// How long a runner whose stdin ended has to go before it's killed.
const KILL_AFTER: Duration = Duration::from_secs(2);
/// The most of its stderr kept for the log, and lines logged.
const STDERR_MAX: usize = 16 * 1024;
const STDERR_LINES: usize = 20;

/// How long the gate has to send its challenge.
const CHALLENGE_WITHIN: Duration = Duration::from_secs(10);

/// Starts the runner; or why it can't.
pub(crate) fn spawn(factory: &Arc<Factory>) -> Result<Io, String> {
    // The browser waits while the vault is locked, as fills do.
    let key = factory.vault.browser_runner_key().map_err(|_| "Claktory's browser waits until your secrets are unlocked.".to_owned())?;
    install().map_err(|error| format!("Claktory couldn't put its browser runner in place: {error}"))?;
    let runner = own_sha256().map_err(|error| format!("Claktory couldn't read its own program: {error}"))?;
    let child = Command::new("sudo")
        .args(["-n", "-u", USER, GATE, "browser-gate", "run"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Claktory couldn't start its browser: {error}"))?;
    pipes(child, Some(Signer { key, runner }))
}

/// What answers the gate: the vault's key, and the hash of the runner it
/// may run, this factoryd's own.
pub(crate) struct Signer {
    key: ed25519_dalek::SigningKey,
    runner: [u8; 32],
}

/// The gate's challenge: its 32 bytes in, the 64-byte signature over them
/// and the runner's hash out, before any frame.
async fn answer(stdin: &mut tokio::process::ChildStdin, stdout: &mut tokio::process::ChildStdout, signer: &Signer) -> Result<(), String> {
    use ed25519_dalek::Signer as _;
    let mut nonce = [0u8; 32];
    match tokio::time::timeout(CHALLENGE_WITHIN, stdout.read_exact(&mut nonce)).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => return Err(format!("the gate sent no challenge: {error}")),
        Err(_) => return Err("the gate sent no challenge in time".to_owned()),
    }
    let signature = signer.key.sign(&gate::message(&nonce, &signer.runner)).to_bytes();
    stdin.write_all(&signature).await.map_err(|error| format!("couldn't answer the gate: {error}"))?;
    stdin.flush().await.map_err(|error| format!("couldn't answer the gate: {error}"))
}

/// This factoryd's program as it runs: on Linux the file the kernel
/// started, even once another has taken its path.
fn own_exe() -> std::io::Result<std::path::PathBuf> {
    if cfg!(target_os = "linux") { Ok(std::path::PathBuf::from("/proc/self/exe")) } else { std::env::current_exe() }
}

/// The SHA-256 of `own_exe`, read once: the only runner factoryd signs for
/// is a copy of itself, hashed by itself.
fn own_sha256() -> std::io::Result<[u8; 32]> {
    static HASH: std::sync::OnceLock<[u8; 32]> = std::sync::OnceLock::new();
    if let Some(hash) = HASH.get() {
        return Ok(*hash);
    }
    let hash = sha256_of(&own_exe()?)?;
    Ok(*HASH.get_or_init(|| hash))
}

fn sha256_of(path: &Path) -> std::io::Result<[u8; 32]> {
    use sha2::Digest;
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hash = sha2::Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hash.finalize().into());
        }
        hash.update(&buffer[..read]);
    }
}

/// C1 over a started runner's stdin and stdout (its stderr summarised):
/// the Linux runner through sudo, or the Mac's through `container` (P6).
/// The child must have all three piped and kill_on_drop. When the host lets
/// go, its stdin ends; a runner still there 2 s later gets SIGTERM (which
/// sudo and `container run` pass on, so the Mac's VM stops too), and 2 s
/// after that SIGKILL. With `signer`, the gate's challenge is answered
/// first; a gate that sends none is killed, and nothing it says is read.
pub(crate) fn pipes(mut child: tokio::process::Child, signer: Option<Signer>) -> Result<Io, String> {
    let (Some(mut stdin), Some(mut stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        return Err("Claktory couldn't start its browser.".to_owned());
    };
    let (to, mut to_runner) = mpsc::channel::<(Channel, Vec<u8>)>(64);
    let (from_runner, from) = mpsc::channel::<(Channel, Vec<u8>)>(64);
    let quit = tokio_util::sync::CancellationToken::new();

    // The gate's challenge first, when there's one; then out, each frame
    // checked against its cap before it's read, and in, until the host
    // lets go (quit, or every sender gone), when the runner's stdin ends,
    // and it's killed if it hasn't gone 2 s later.
    let quitting = quit.clone();
    tokio::spawn(async move {
        if let Some(signer) = signer
            && let Err(why) = answer(&mut stdin, &mut stdout, &signer).await
        {
            tracing::warn!(%why, "Claktory's browser gate wasn't answered: stopped");
            let _ = child.start_kill();
            let _ = child.wait().await;
            return;
        }
        tokio::spawn(frames_out(stdout, from_runner));
        loop {
            let next = tokio::select! {
                biased;
                () = quitting.cancelled() => None,
                next = to_runner.recv() => next,
            };
            let Some((channel, payload)) = next else { break };
            let Ok(header) = wire::header(channel, payload.len(), Way::In) else { continue };
            if stdin.write_all(&header).await.is_err() || stdin.write_all(&payload).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
        drop(stdin);
        if tokio::time::timeout(KILL_AFTER, child.wait()).await.is_ok() {
            return;
        }
        #[cfg(unix)]
        if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
            // SAFETY: a plain signal to the child we started and still hold.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            if tokio::time::timeout(KILL_AFTER, child.wait()).await.is_ok() {
                return;
            }
        }
        let _ = child.start_kill();
        let _ = child.wait().await;
    });

    tokio::spawn(stderr_summary(stderr));
    Ok(Io { to, from, quit })
}

/// The runner's frames, each checked against its cap before it's read.
async fn frames_out(mut stdout: tokio::process::ChildStdout, from_runner: mpsc::Sender<(Channel, Vec<u8>)>) {
    loop {
        let mut head = [0u8; wire::HEADER];
        if stdout.read_exact(&mut head).await.is_err() {
            return;
        }
        let (channel, len) = match wire::parse_header(head, Way::Out) {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!(%error, "Claktory's browser runner sent a frame out of bounds: closed");
                return;
            }
        };
        let mut payload = vec![0u8; len];
        if stdout.read_exact(&mut payload).await.is_err() || from_runner.send((channel, payload)).await.is_err() {
            return;
        }
    }
}

/// The runner's stderr: read capped, logged cleaned and cut.
async fn stderr_summary(stderr: tokio::process::ChildStderr) {
    let mut kept = Vec::new();
    let _ = stderr.take(STDERR_MAX as u64).read_to_end(&mut kept).await;
    for line in String::from_utf8_lossy(&kept).lines().filter(|line| !line.trim().is_empty()).take(STDERR_LINES) {
        tracing::info!(line = %super::view::cleaned(line, 200), "browser runner");
    }
}

/// Puts this factoryd at the runner's path when what's there differs:
/// written beside it, then renamed over it, so a runner starting meanwhile
/// never sees half of it.
fn install() -> std::io::Result<()> {
    let me = own_exe()?;
    let runner = Path::new(RUNNER);
    if same(&me, runner) {
        return Ok(());
    }
    let staging = runner.with_extension("new");
    // A stale one from a copy cut short goes first; the new one is made
    // afresh (never an existing file truncated and written through).
    match std::fs::remove_file(&staging) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o755);
    }
    let mut to = options.open(&staging)?;
    std::io::copy(&mut std::fs::File::open(&me)?, &mut to)?;
    to.sync_all()?;
    drop(to);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&staging, runner)
}

/// Whether two files hold the same bytes.
fn same(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(meta_a), Ok(meta_b)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if meta_a.len() != meta_b.len() {
        return false;
    }
    let (Ok(mut file_a), Ok(mut file_b)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    let (mut buf_a, mut buf_b) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let Ok(read) = file_a.read(&mut buf_a) else { return false };
        if read == 0 {
            return true;
        }
        if file_b.read_exact(&mut buf_b[..read]).is_err() || buf_a[..read] != buf_b[..read] {
            return false;
        }
    }
}

// Claktory's browser isn't on Windows (`setup::check` says so).
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A stand-in gate: sends 32 zero bytes as its challenge, keeps the 64
    /// it gets back in `answer`, then echoes its stdin like cat.
    fn challenger(answer: &Path) -> tokio::process::Child {
        tokio::process::Command::new("/bin/sh")
            .args(["-c", "head -c 32 /dev/zero; dd bs=64 count=1 of=\"$0\" 2>/dev/null; exec cat"])
            .arg(answer)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }

    /// factoryd answers the gate's challenge with its signature over it
    /// and the runner's hash, before any frame; a gate that sends none is
    /// stopped, and nothing it says is read.
    #[tokio::test]
    async fn the_gates_challenge_is_answered_before_any_frame() {
        let dir = std::env::temp_dir().join(format!("gate-answer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let answer = dir.join("answer");
        let key = ed25519_dalek::SigningKey::from_bytes(&[4; 32]);
        let runner = [3u8; 32];
        let Io { to, mut from, quit } = pipes(challenger(&answer), Some(Signer { key: key.clone(), runner })).unwrap();
        to.send((Channel::Control, b"{}".to_vec())).await.unwrap();
        let back = tokio::time::timeout(Duration::from_secs(10), from.recv()).await.expect("in time");
        assert!(back.is_some(), "the frame came back after the challenge");
        let signed: [u8; 64] = std::fs::read(&answer).unwrap().try_into().expect("64 bytes");
        assert!(gate::verify(&key.verifying_key(), &[0; 32], &runner, &signed));
        quit.cancel();

        let silent = tokio::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let Io { mut from, .. } = pipes(silent, Some(Signer { key, runner })).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(10), from.recv()).await.expect("in time").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Quit ends the runner even while a copy of its sender is still held
    /// somewhere (Ghiaccio 38462: an errand's connection kept one, and a
    /// stopped runner ran on).
    #[tokio::test]
    async fn quit_ends_the_runner_whoever_still_holds_its_sender() {
        let child = tokio::process::Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let Io { to, mut from, quit } = pipes(child, None).unwrap();
        let _kept = to.clone();
        drop(to);
        quit.cancel();
        // cat sees its stdin end and exits: its output closes.
        let end = tokio::time::timeout(Duration::from_secs(10), from.recv()).await.expect("the runner ended");
        assert!(end.is_none());
    }
}
