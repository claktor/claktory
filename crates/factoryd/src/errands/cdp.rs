//! C2 (#435, SPLIT.md): the errands' Chrome DevTools Protocol client, over
//! a [`Transport`], so the same errand drives a local headless Chrome (tests,
//! and the Mac dev backend: [`Local`]) or the browser runner's driven Chrome
//! on a Linux server (P4's `browser_host::Handle`, over C1's CDP channel).
//!
//! Everything that comes back is a hostile page's, after a Chrome escape
//! (Fugo 36536, 36539): a message is capped at [`MAX_MESSAGE`] before it's
//! kept, parsed by serde_json (its nesting held to 128 levels), and one that
//! isn't a reply or an event is dropped, never trusted.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use std::sync::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{broadcast, mpsc, oneshot};

/// The most one CDP message takes, either way: C1's cap for its channel.
pub(crate) const MAX_MESSAGE: usize = crate::browser_runner::wire::MAX_CDP;

/// How long a call waits for its reply unless told otherwise.
pub(crate) const WAIT: Duration = Duration::from_secs(30);

/// One CDP connection: whole messages (JSON, unframed), each at most
/// [`MAX_MESSAGE`], both ways. The transport frames them on its wire (NUL
/// after each over Chrome's pipe; C1's channel 1 to the runner), and holds
/// what it receives to the cap before keeping it. `receive` ends when the
/// browser goes; dropping `send` closes the connection.
pub(crate) struct Pipe {
    pub send: mpsc::Sender<Vec<u8>>,
    pub receive: mpsc::Receiver<Vec<u8>>,
}

/// Where an errand's Chrome is (C2): implemented by [`Local`] and by P4's
/// `browser_host::Handle`, which also offers `set_mode(driven|undriven,
/// url)`, `stop()`, `state()` and a watch on its state.
pub(crate) trait Transport: Send + Sync {
    /// The CDP connection to the driven Chrome, one at a time: Err while
    /// it's undriven or gone, or while another connection is open, saying
    /// so in a line the agent may read.
    fn connect(&self) -> Result<Pipe, String>;
}

/// What a call can come to.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Error {
    #[error("the browser closed")]
    Closed,
    #[error("{0} had no answer in {1} s")]
    Timeout(String, u64),
    /// Chrome's own refusal, its message as it said it.
    #[error("{0}: {1}")]
    Refused(String, String),
    #[error("{0} is too big to send to the browser")]
    TooBig(String),
}

/// An event Chrome sent: its method, its params, and the session (a page
/// attached with `flatten`) it came from, if any.
#[derive(Debug, Clone)]
pub(crate) struct Event {
    pub method: String,
    pub params: Value,
    pub session: Option<String>,
}

type Waiting = Arc<Mutex<HashMap<u64, (String, oneshot::Sender<Result<Value, Error>>)>>>;

/// A CDP client: calls matched to their replies by id, events to whoever
/// listens.
pub(crate) struct Cdp {
    send: mpsc::Sender<Vec<u8>>,
    next: AtomicU64,
    waiting: Waiting,
    events: broadcast::Sender<Event>,
    closed: Arc<AtomicBool>,
}

/// A message from Chrome, as far as the client reads it.
#[derive(Deserialize)]
struct Incoming {
    id: Option<u64>,
    result: Option<Value>,
    error: Option<Refusal>,
    method: Option<String>,
    params: Option<Value>,
    #[serde(rename = "sessionId")]
    session: Option<String>,
}

#[derive(Deserialize)]
struct Refusal {
    #[serde(default)]
    message: String,
}

/// The most a refusal's message keeps, in characters: it's the page's
/// words as often as Chrome's.
const MAX_REFUSAL: usize = 500;

impl Cdp {
    /// A client over `pipe`, reading it until it ends.
    pub(crate) fn new(pipe: Pipe) -> Cdp {
        let Pipe { send, mut receive } = pipe;
        let waiting: Waiting = Arc::default();
        let (events, _) = broadcast::channel(256);
        let closed = Arc::new(AtomicBool::new(false));
        let reader = (waiting.clone(), events.clone(), closed.clone());
        tokio::spawn(async move {
            let (waiting, events, closed) = reader;
            while let Some(message) = receive.recv().await {
                if message.len() > MAX_MESSAGE {
                    break;
                }
                let Ok(incoming) = serde_json::from_slice::<Incoming>(&message) else {
                    continue;
                };
                drop(message);
                if let Some(id) = incoming.id {
                    let Some((method, reply)) = crate::locking::lock(&waiting).remove(&id) else {
                        continue;
                    };
                    let outcome = match incoming.error {
                        Some(refusal) => Err(Error::Refused(
                            method,
                            refusal.message.chars().take(MAX_REFUSAL).collect(),
                        )),
                        None => Ok(incoming.result.unwrap_or(Value::Null)),
                    };
                    let _ = reply.send(outcome);
                } else if let Some(method) = incoming.method {
                    let _ = events.send(Event {
                        method,
                        params: incoming.params.unwrap_or(Value::Null),
                        session: incoming.session,
                    });
                }
            }
            closed.store(true, Ordering::SeqCst);
            for (_, (_, reply)) in crate::locking::lock(&waiting).drain() {
                let _ = reply.send(Err(Error::Closed));
            }
        });
        Cdp { send, next: AtomicU64::new(1), waiting, events, closed }
    }

    /// Calls `method` with `params`, in `session` when given, and waits up
    /// to [`WAIT`] for its result.
    pub(crate) async fn call(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> Result<Value, Error> {
        self.call_within(method, params, session, WAIT).await
    }

    /// [`Cdp::call`], waiting up to `wait`.
    pub(crate) async fn call_within(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
        wait: Duration,
    ) -> Result<Value, Error> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session) = session {
            message["sessionId"] = Value::String(session.to_owned());
        }
        let bytes = serde_json::to_vec(&message).map_err(|_| Error::TooBig(method.to_owned()))?;
        if bytes.len() > MAX_MESSAGE {
            return Err(Error::TooBig(method.to_owned()));
        }
        let (reply, answer) = oneshot::channel();
        crate::locking::lock(&self.waiting).insert(id, (method.to_owned(), reply));
        // Checked after the insert: a reader that ends meanwhile either is
        // seen here or finds this call to end.
        let sent_and_answered = async {
            if self.closed.load(Ordering::SeqCst) || self.send.send(bytes).await.is_err() {
                return Err(Error::Closed);
            }
            answer.await.unwrap_or(Err(Error::Closed))
        };
        let outcome = tokio::time::timeout(wait, sent_and_answered)
            .await
            .unwrap_or_else(|_| Err(Error::Timeout(method.to_owned(), wait.as_secs())));
        if outcome.is_err() {
            crate::locking::lock(&self.waiting).remove(&id);
        }
        outcome
    }

    /// Chrome's events from now on. A listener that falls 256 behind
    /// loses the oldest (broadcast's Lagged).
    pub(crate) fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Whether the browser went.
    pub(crate) fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// The Chrome to drive here: CHROME, else the newest Chrome for Testing in
/// ~/.cache/chrome-for-testing (as browser-extension/test/cdp.mjs looks),
/// else Google Chrome or Chromium where they install.
pub(crate) fn find_chrome() -> Option<PathBuf> {
    if let Some(chrome) = std::env::var_os("CHROME") {
        return Some(PathBuf::from(chrome));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(root) = home.map(|home| home.join(".cache/chrome-for-testing/chrome")) {
        let mut versions: Vec<PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        versions.sort();
        for version in versions.iter().rev() {
            for binary in [
                "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
                "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
                "chrome-linux64/chrome",
            ] {
                let file = version.join(binary);
                if file.is_file() {
                    return Some(file);
                }
            }
        }
    }
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/usr/bin/google-chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|file| file.is_file())
}

/// A headless Chrome on this machine, driven over `--remote-debugging-pipe`
/// (it reads CDP on fd 3 and writes on fd 4, each message followed by a
/// NUL), in a profile of its own. Its process group is killed when it's
/// dropped.
pub(crate) struct Local {
    pipe: Mutex<Option<Pipe>>,
    group: i32,
    _child: tokio::process::Child,
}

impl Local {
    /// Starts `chrome` headless on about:blank in `profile`.
    pub(crate) fn start(chrome: &Path, profile: &Path) -> std::io::Result<Local> {
        use std::os::fd::{AsRawFd, OwnedFd};

        let (chrome_reads, we_write) = std::io::pipe()?;
        let (we_read, chrome_writes) = std::io::pipe()?;
        let (into, out) = (chrome_reads.as_raw_fd(), chrome_writes.as_raw_fd());
        let mut command = tokio::process::Command::new(chrome);
        command
            .args([
                "--headless=new",
                "--remote-debugging-pipe",
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-sync",
                "--use-mock-keychain",
                "--password-store=basic",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .kill_on_drop(true);
        // SAFETY: only async-signal-safe calls (fcntl, dup2) between fork
        // and exec. Both ends are moved above 9 first, so placing one on 3
        // or 4 can't overwrite the other; dup2 clears close-on-exec on the
        // copies, and every other descriptor of ours keeps it.
        unsafe {
            command.pre_exec(move || {
                let into = libc::fcntl(into, libc::F_DUPFD_CLOEXEC, 10);
                let out = libc::fcntl(out, libc::F_DUPFD_CLOEXEC, 10);
                if into < 0 || out < 0 || libc::dup2(into, 3) < 0 || libc::dup2(out, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        drop((chrome_reads, chrome_writes));
        let group = child.id().map(|pid| pid as i32).unwrap_or(0);
        let writer = tokio::net::unix::pipe::Sender::from_owned_fd(OwnedFd::from(we_write))?;
        let reader = tokio::net::unix::pipe::Receiver::from_owned_fd(OwnedFd::from(we_read))?;
        let (send, mut outgoing) = mpsc::channel::<Vec<u8>>(64);
        let (incoming, receive) = mpsc::channel::<Vec<u8>>(64);
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let mut writer = writer;
            while let Some(mut message) = outgoing.recv().await {
                message.push(0);
                if writer.write_all(&message).await.is_err() {
                    break;
                }
            }
        });
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut reader = reader;
            let mut kept: Vec<u8> = Vec::new();
            let mut chunk = vec![0u8; 64 * 1024];
            while let Ok(read) = reader.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                let mut rest = &chunk[..read];
                while let Some(end) = rest.iter().position(|&byte| byte == 0) {
                    if kept.len() + end > MAX_MESSAGE {
                        return;
                    }
                    kept.extend_from_slice(&rest[..end]);
                    if incoming.send(std::mem::take(&mut kept)).await.is_err() {
                        return;
                    }
                    rest = &rest[end + 1..];
                }
                if kept.len() + rest.len() > MAX_MESSAGE {
                    return;
                }
                kept.extend_from_slice(rest);
            }
        });
        Ok(Local { pipe: Mutex::new(Some(Pipe { send, receive })), group, _child: child })
    }
}

impl Transport for Local {
    fn connect(&self) -> Result<Pipe, String> {
        crate::locking::lock(&self.pipe)
            .take()
            .ok_or_else(|| "The browser is already being driven.".to_owned())
    }
}

impl Drop for Local {
    fn drop(&mut self) {
        if self.group > 0 {
            // SAFETY: a plain signal to the group Chrome leads.
            unsafe {
                libc::killpg(self.group, libc::SIGKILL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pipe this test plays Chrome on.
    fn played() -> (Cdp, mpsc::Receiver<Vec<u8>>, mpsc::Sender<Vec<u8>>) {
        let (send, sent) = mpsc::channel(16);
        let (reply, receive) = mpsc::channel(16);
        (Cdp::new(Pipe { send, receive }), sent, reply)
    }

    #[tokio::test]
    async fn calls_get_their_own_replies_and_events_reach_listeners() {
        let (cdp, mut sent, reply) = played();
        let mut events = cdp.events();
        let chrome = tokio::spawn(async move {
            let first: Value = serde_json::from_slice(&sent.recv().await.unwrap()).unwrap();
            let second: Value = serde_json::from_slice(&sent.recv().await.unwrap()).unwrap();
            assert_eq!(first["method"], "Browser.getVersion");
            assert_eq!(second["sessionId"], "S");
            for message in [
                json!({"method": "Page.loadEventFired", "params": {"t": 1}, "sessionId": "S"}),
                json!({"id": 999, "result": {}}),
                json!("not a message"),
                json!({"id": second["id"], "error": {"code": -32000, "message": "No node"}}),
                json!({"id": first["id"], "result": {"product": "Chrome"}}),
            ] {
                reply.send(serde_json::to_vec(&message).unwrap()).await.unwrap();
            }
            reply.send(b"{\"id\":".to_vec()).await.unwrap();
            reply
        });
        let (version, node) = tokio::join!(
            cdp.call("Browser.getVersion", json!({}), None),
            cdp.call("DOM.describeNode", json!({}), Some("S")),
        );
        assert_eq!(version.unwrap()["product"], "Chrome");
        assert_eq!(node, Err(Error::Refused("DOM.describeNode".into(), "No node".into())));
        let event = events.recv().await.unwrap();
        assert_eq!((event.method.as_str(), event.session.as_deref()), ("Page.loadEventFired", Some("S")));
        assert_eq!(event.params["t"], 1);
        let reply = chrome.await.unwrap();
        assert!(!cdp.closed());
        drop(reply);
    }

    #[tokio::test]
    async fn a_browser_that_goes_ends_every_call_and_one_that_stays_silent_times_out() {
        let (cdp, mut sent, reply) = played();
        let silent = cdp.call_within("Page.navigate", json!({}), None, Duration::from_millis(50));
        assert_eq!(silent.await, Err(Error::Timeout("Page.navigate".into(), 0)));
        assert!(crate::locking::lock(&cdp.waiting).is_empty());
        let going = tokio::spawn(async move {
            sent.recv().await.unwrap();
            drop(reply);
            sent
        });
        assert_eq!(cdp.call("Runtime.evaluate", json!({}), None).await, Err(Error::Closed));
        let _sent = going.await.unwrap();
        assert!(cdp.closed());
        assert_eq!(cdp.call("Runtime.evaluate", json!({}), None).await, Err(Error::Closed));
    }

    #[tokio::test]
    async fn nothing_too_big_is_sent_and_a_reply_too_big_closes_the_connection() {
        let (cdp, _sent, reply) = played();
        let huge = "x".repeat(MAX_MESSAGE);
        let refused = cdp.call("Runtime.evaluate", json!({"expression": huge}), None).await;
        assert_eq!(refused, Err(Error::TooBig("Runtime.evaluate".into())));
        reply.send(vec![b' '; MAX_MESSAGE + 1]).await.unwrap();
        assert_eq!(cdp.call("Browser.getVersion", json!({}), None).await, Err(Error::Closed));
    }

    /// Drives a real headless Chrome over its pipe: skipped where there's
    /// none (CHROME names one).
    #[tokio::test]
    async fn a_local_chrome_is_driven_over_its_pipe() {
        let Some(chrome) = find_chrome() else {
            eprintln!("no Chrome here: skipped");
            return;
        };
        let profile = std::env::temp_dir().join(format!("factoryd-cdp-{}", std::process::id()));
        std::fs::create_dir_all(&profile).unwrap();
        let local = Local::start(&chrome, &profile).unwrap();
        let cdp = Cdp::new(local.connect().unwrap());
        assert!(local.connect().is_err());
        let version = cdp.call("Browser.getVersion", json!({}), None).await.unwrap();
        assert!(version["product"].as_str().unwrap().contains("Chrome"), "{version}");
        let target = cdp
            .call("Target.createTarget", json!({"url": "about:blank"}), None)
            .await
            .unwrap();
        let session = cdp
            .call(
                "Target.attachToTarget",
                json!({"targetId": target["targetId"], "flatten": true}),
                None,
            )
            .await
            .unwrap();
        let session = session["sessionId"].as_str().unwrap();
        let sum = cdp
            .call(
                "Runtime.evaluate",
                json!({"expression": "6 * 7", "returnByValue": true}),
                Some(session),
            )
            .await
            .unwrap();
        assert_eq!(sum["result"]["value"], 42);
        let wrong = cdp.call("No.suchMethod", json!({}), None).await;
        assert!(matches!(wrong, Err(Error::Refused(..))), "{wrong:?}");
        let group = local.group;
        drop(local);
        assert_eq!(cdp.call("Browser.getVersion", json!({}), None).await, Err(Error::Closed));
        // SAFETY: signal 0 only asks whether the group is still there.
        let gone = (0..50).any(|_| {
            std::thread::sleep(Duration::from_millis(100));
            (unsafe { libc::killpg(group, 0) }) != 0
        });
        assert!(gone, "Chrome's group outlived its Local");
        let _ = std::fs::remove_dir_all(&profile);
    }
}
