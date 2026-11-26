//! The runner on Linux (#435 P4): `factoryd browser-runner`, copied to
//! /opt/claktory-browser/runner when it differs, then run as the browser's
//! own user through the one sudo line setup installed (C4). Its stdin and
//! stdout carry C1; nothing else is shared: sudo closes every other file
//! descriptor, and its environment is sudo's own.
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
use super::setup::{RUNNER, USER};
use crate::browser_runner::wire::{self, Channel, Way};
use crate::factory::Factory;

/// How long a runner whose stdin ended has to go before it's killed.
const KILL_AFTER: Duration = Duration::from_secs(2);
/// The most of its stderr kept for the log, and lines logged.
const STDERR_MAX: usize = 16 * 1024;
const STDERR_LINES: usize = 20;

/// Starts the runner; or why it can't.
pub(crate) fn spawn(_factory: &Arc<Factory>) -> Result<Io, String> {
    install().map_err(|error| format!("Claktory couldn't put its browser runner in place: {error}"))?;
    let child = Command::new("sudo")
        .args(["-n", "-u", USER, RUNNER, "browser-runner", "run"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("Claktory couldn't start its browser: {error}"))?;
    pipes(child)
}

/// C1 over a started runner's stdin and stdout (its stderr summarised):
/// the Linux runner through sudo, or the Mac's through `container` (P6).
/// The child must have all three piped and kill_on_drop. When the host lets
/// go, its stdin ends; a runner still there 2 s later gets SIGTERM (which
/// sudo and `container run` pass on, so the Mac's VM stops too), and 2 s
/// after that SIGKILL.
pub(crate) fn pipes(mut child: tokio::process::Child) -> Result<Io, String> {
    let (Some(mut stdin), Some(mut stdout), Some(stderr)) = (child.stdin.take(), child.stdout.take(), child.stderr.take()) else {
        return Err("Claktory couldn't start its browser.".to_owned());
    };
    let (to, mut to_runner) = mpsc::channel::<(Channel, Vec<u8>)>(64);
    let (from_runner, from) = mpsc::channel::<(Channel, Vec<u8>)>(64);
    let quit = tokio_util::sync::CancellationToken::new();

    // In: until the host lets go (quit, or every sender gone), then the
    // runner's stdin ends, and it's killed if it hasn't gone 2 s later.
    let quitting = quit.clone();
    tokio::spawn(async move {
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

    // Out: each frame checked against its cap before it's read.
    tokio::spawn(async move {
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
    });

    tokio::spawn(stderr_summary(stderr));
    Ok(Io { to, from, quit })
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
    let me = std::env::current_exe()?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let Io { to, mut from, quit } = pipes(child).unwrap();
        let _kept = to.clone();
        drop(to);
        quit.cancel();
        // cat sees its stdin end and exits: its output closes.
        let end = tokio::time::timeout(Duration::from_secs(10), from.recv()).await.expect("the runner ended");
        assert!(end.is_none());
    }
}
